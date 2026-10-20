// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The filing of a message in a different mailbox, as an `Email/set`.
//!
//! Camel uses one vfunc, `transfer_messages_to_sync`, with a flag that tells
//! whether to remove the originals. JMAP uses one property for both: RFC 8621
//! §4.6 makes `mailboxIds` the set of mailboxes of a message. A copy adds a
//! member. A move adds one member and removes a different member.
//!
//! The main rule is from RFC 8621 §4.6: an `Email` must be in at least one
//! mailbox. Thus a move is one patch, not two requests.

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

    /// Seed a mailbox with the given name.
    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, None)
    }

    /// Seed one message in the given mailbox.
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
}

#[test]
fn a_copy_names_only_the_mailbox_it_is_filed_into() {
    // A copy adds only one member. It must not change the other mailboxes of
    // the message.
    let filing = Filing::copied_into(Id::new("M2"));

    assert!(!filing.is_empty());
    assert_eq!(filing.patch(), json!({"mailboxIds/M2": true}));
}

#[test]
fn a_move_takes_the_message_out_and_files_it_in_one_patch() {
    // One patch, not two requests: RFC 8621 §4.6 puts an Email in one or more
    // Mailboxes. If the removal is first, the server refuses it, because no
    // mailbox stays. If the addition is first and the removal fails, the
    // message stays in two mailboxes.
    let filing = Filing::moved(Id::new("M1"), Id::new("M2"));

    assert_eq!(
        filing.patch(),
        json!({"mailboxIds/M2": true, "mailboxIds/M1": null})
    );
}

#[test]
fn a_move_into_the_mailbox_the_message_is_already_in_says_nothing() {
    // A patch cannot show this filing: one pointer must be `true` and `null`.
    // No change is necessary, so the client sends nothing.
    let filing = Filing::moved(Id::new("M1"), Id::new("M1"));

    assert!(filing.is_empty());
    assert_eq!(filing.patch(), json!({}));
}

#[test]
fn a_mailbox_id_with_a_pointer_character_in_it_names_one_mailbox() {
    // RFC 8620 §1.2 permits only URL-safe characters in an id, thus no `/` or
    // `~`. But the id comes from the network, and a patch key is a JSON Pointer
    // (RFC 8620 §5.3, RFC 6901). Without escapes, an id `a/b` lets the server
    // select which Email property the client patches.
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

    // The account does not have this id, so a request gets `notFound`. Success
    // proves that no request was sent.
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

    // The same rule as `set_keywords` and `message_source`: a summary uid
    // comes from the last listing. Destruction by a different client is normal.
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

    // Not `NoSuchMessage`: the message exists, but the destination does not.
    // Usually, Camel shows a folder that the server deleted. The user must
    // get the error.
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
    // Check the main rule of `Filing` against the server: RFC 8621 §4.6 puts an
    // Email in one or more Mailboxes. Thus no server accepts the first patch of
    // a two-request move. Send it raw, because `Filing` cannot make it.
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
