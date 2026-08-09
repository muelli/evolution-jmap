// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a message is filed, as the `Email/set` that files it somewhere else.
//!
//! Camel asks for this with one vfunc — `transfer_messages_to_sync`, with a
//! flag saying whether the originals go — and JMAP answers both halves of it
//! with the same property: RFC 8621 §4.6 makes `mailboxIds` the set of
//! mailboxes a message is in, so a copy adds a member and a move adds one and
//! takes another away.
//!
//! The rule the whole module is built around is the one sentence RFC 8621 §4.6
//! spends on it: an `Email` must be in at least one mailbox. That is why a move
//! is one patch rather than two requests, and why a move into the mailbox the
//! message already sits in is not a request at all.

use std::collections::BTreeMap;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Filing, MailSync, SyncError};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use serde_json::json;

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

    /// A mailbox of the account, by name.
    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, None)
    }

    /// One message, in the mailbox given.
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
fn a_copy_names_only_the_mailbox_it_is_filed_into() {
    // A copy says nothing about where the message already is: the member it
    // adds is the whole of it, and every other mailbox the message is in is one
    // this side has no business speaking for.
    let filing = Filing::copied_into(Id::new("M2"));

    assert!(!filing.is_empty());
    assert_eq!(filing.patch(), json!({"mailboxIds/M2": true}));
}

#[test]
fn a_move_takes_the_message_out_and_files_it_in_one_patch() {
    // One patch and not two requests, because RFC 8621 §4.6 says an Email
    // belongs to one or more Mailboxes: a request that removed first would be
    // refused for leaving it in none, and one that added first would leave the
    // message filed twice if the second request never happened.
    let filing = Filing::moved(Id::new("M1"), Id::new("M2"));

    assert_eq!(
        filing.patch(),
        json!({"mailboxIds/M2": true, "mailboxIds/M1": null})
    );
}

#[test]
fn a_move_into_the_mailbox_the_message_is_already_in_says_nothing() {
    // The one filing that cannot be expressed as a patch: the same pointer
    // would have to be both `true` and `null`, and whichever won, the answer
    // would be a message either filed where it already was or filed nowhere.
    // Nothing has to happen, so nothing is sent.
    let filing = Filing::moved(Id::new("M1"), Id::new("M1"));

    assert!(filing.is_empty());
    assert_eq!(filing.patch(), json!({}));
}

#[test]
fn a_mailbox_id_with_a_pointer_character_in_it_names_one_mailbox() {
    // RFC 8620 §1.2 limits an id to URL-safe characters, so `/` and `~` cannot
    // appear in one — but the id in hand came off the network, and a patch key
    // is a JSON Pointer (RFC 8620 §5.3, RFC 6901). Unescaped, a server that
    // answered with `a/b` would be a server that chooses which property of an
    // Email this client patches.
    let filing = Filing::copied_into(Id::new("a/b~c"));

    assert_eq!(filing.patch(), json!({"mailboxIds/a~1b~0c": true}));
}

#[test]
fn copying_a_message_leaves_it_where_it_was() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&inbox);

    fixture
        .sync()
        .file_message(&uid, &Filing::copied_into(archive.clone()))
        .unwrap();

    assert_eq!(
        fixture.mailboxes_on_server(&uid),
        BTreeMap::from([(inbox, true), (archive, true)])
    );
}

#[test]
fn moving_a_message_takes_it_out_of_the_mailbox_it_came_from() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&inbox);

    fixture
        .sync()
        .file_message(&uid, &Filing::moved(inbox, archive.clone()))
        .unwrap();

    assert_eq!(
        fixture.mailboxes_on_server(&uid),
        BTreeMap::from([(archive, true)])
    );
}

#[test]
fn a_filing_that_says_nothing_is_not_a_request() {
    let fixture = Fixture::start();

    // An id the account has never held: a request would come back `notFound`,
    // so succeeding is what proves none was sent.
    fixture
        .sync()
        .file_message(
            &Id::new("E404"),
            &Filing::moved(Id::new("M1"), Id::new("M1")),
        )
        .expect("a filing that says nothing needs no server");
}

#[test]
fn a_message_another_client_deleted_is_reported_as_gone() {
    let fixture = Fixture::start();
    let archive = fixture.seed_mailbox("Archive");

    let error = fixture
        .sync()
        .file_message(&Id::new("E404"), &Filing::copied_into(archive))
        .expect_err("a message that is not there cannot be filed");

    // The same judgement `set_keywords` and `message_source` make about the
    // same situation: a uid in a folder summary is a claim about the last
    // listing, and another client destroying the message is ordinary.
    match error {
        SyncError::NoSuchMessage(uid) => assert_eq!(uid.as_str(), "E404"),
        other => panic!("expected the message to be reported as gone, got {other:?}"),
    }
}

#[test]
fn filing_into_a_mailbox_the_account_does_not_have_is_refused() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);

    let error = fixture
        .sync()
        .file_message(&uid, &Filing::copied_into(Id::new("M404")))
        .expect_err("a mailbox that is not there cannot hold a message");

    // Not `NoSuchMessage`: the message is fine, the destination is not. A
    // folder Camel still shows and the server has already deleted is the
    // ordinary way to reach this, and the user has to be told.
    match error {
        SyncError::Client(jmap_client::Error::Set(set_error)) => {
            assert_eq!(
                set_error.error_type,
                jmap_proto::error::set::INVALID_PROPERTIES
            );
        }
        other => panic!("expected the server's own refusal, got {other:?}"),
    }
    assert_eq!(
        fixture.mailboxes_on_server(&uid),
        BTreeMap::from([(inbox, true)])
    );
}

#[test]
fn a_message_may_not_be_left_in_no_mailbox_at_all() {
    // The rule `Filing` is built around, asserted against the server rather
    // than assumed: RFC 8621 §4.6 has an Email belong to one or more Mailboxes,
    // so the patch a two-request move would send first is one no server may
    // accept. Sent raw, because `Filing` cannot express it.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);

    let sync = fixture.sync();
    let error = sync
        .client()
        .email_update(
            sync.account_id(),
            &uid,
            json!({ format!("mailboxIds/{inbox}"): null }),
        )
        .expect_err("a message in no mailbox is not a message the server keeps");

    match error {
        jmap_client::Error::Set(set_error) => {
            assert_eq!(
                set_error.error_type,
                jmap_proto::error::set::INVALID_PROPERTIES
            );
        }
        other => panic!("expected the server's own refusal, got {other:?}"),
    }
    assert_eq!(
        fixture.mailboxes_on_server(&uid),
        BTreeMap::from([(inbox, true)])
    );
}
