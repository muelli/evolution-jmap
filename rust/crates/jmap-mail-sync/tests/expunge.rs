// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The permanent removal of a message from one mailbox: the write behind
//! `expunge_sync` in Camel.
//!
//! This write has two requests. `camel_folder_expunge_sync` tells a *folder*
//! to remove the messages that the user marked as deleted *in that folder*. In
//! IMAP, a message is in exactly one mailbox, so this is clear. In JMAP, RFC
//! 8621 §4.6 makes `mailboxIds` a set, so one message can be in the inbox and
//! in a folder of the user. The two possible writes have different results:
//!
//! - `Email/set` **destroy** removes the message from the account. This is
//!   correct when this mailbox is the last one. Otherwise, it deletes data: an
//!   empty trash also removes the copy in "Work".
//! - `Email/set` **update** with `mailboxIds/<this>: null` removes the message
//!   from this mailbox only. This is correct for the second case. For the
//!   first case, a server that keeps the RFC 8621 §4.6 invariant refuses it,
//!   because the message then has no mailbox.
//!
//! Camel cannot know which case applies: a summary row records only the
//! mailbox of its listing. Thus the code reads the mailboxes first, then
//! selects the write from the answer. These tests examine that sequence.

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

    /// Whether the account still has the message.
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

    /// The mailboxes of the message on the server now.
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

    /// Overwrite the `mailboxIds` of a seeded message in the store. This makes
    /// states that a conforming server never sends (RFC 8621 §4.6 makes each
    /// present value `true`).
    fn set_mailbox_ids(&self, uid: &Id, mailbox_ids: BTreeMap<Id, bool>) {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        account
            .emails
            .get_mut(uid)
            .expect("the seeded message")
            .mailbox_ids = Some(mailbox_ids);
    }
}

#[test]
fn a_message_this_mailbox_is_the_last_home_of_is_destroyed() {
    // The usual case, as in "empty the trash": the message is only in this
    // mailbox. Removal from the mailbox and destruction then have the same
    // result, but RFC 8621 §4.6 permits only destruction.
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
    // This case is the reason for the read. The user copied the message to a
    // folder, then deleted it from the inbox. Destruction also removes the copy.
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
    // A summary row records the last listing. A different client can move the
    // message while Evolution has the folder open. Then the message is already
    // out of this mailbox, so the expunge does nothing. A destruction based on
    // an old row deletes mail.
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
    // All writes in this crate apply the same rule: a uid comes from the last
    // listing. If a different client destroyed the message, the expunge has
    // no work, and it is not an error.
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
    // The same expunge again, as in a retry after a partial failure. Thus the
    // caller can remove the row on NoSuchMessage and show no error.
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

#[test]
fn a_false_valued_mailbox_entry_is_refused_rather_than_guessed_at() {
    // RFC 8621 §4.6 makes each present `mailboxIds` value `true`. A `false`
    // value violates the RFC, and neither write is safe:
    // - As membership, the entry turns a message in one mailbox into an update
    //   that leaves it in no mailbox, which a conforming server refuses.
    // - Ignored, the entry can cause destruction of a copy.
    // Thus the code refuses it at the start, as `identity_for` refuses an
    // identity without an id.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&inbox);
    fixture.set_mailbox_ids(
        &uid,
        BTreeMap::from([(inbox.clone(), true), (archive, false)]),
    );

    let error = fixture
        .sync()
        .expunge_message(&uid, &inbox)
        .expect_err("a false-valued mailboxIds entry must not be guessed at");

    match error {
        SyncError::Client(jmap_client::Error::Protocol(message)) => {
            assert!(
                message.contains("false-valued"),
                "expected a message naming the false-valued entry, got: {message}"
            );
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
    assert!(
        fixture.holds(&uid),
        "a refused expunge must leave the message untouched"
    );
}
