// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Importing a message: uploading RFC 5322 bytes and having the account make an
//! `Email` of them (`Email/import`, RFC 8621 §4.8).
//!
//! This is the other way a message gets into the store, and the one a mail
//! client needs: an `Email/set` create builds a message out of properties, which
//! is composing, while a message Evolution already holds — a draft it wrote, a
//! message dragged out of another account — is bytes, and an import is how it
//! arrives unaltered. It is what `append_message_sync` will be.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::error::set;
use jmap_proto::mail::{EmailImport, EmailImportRequest, email_import_error, keyword, role};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_MAIL};
use jmap_proto::{Id, UtcDate};

/// A whole small message, headers and body, as a client would serialize one.
///
/// Written out here rather than assembled by a helper because the assertions
/// below are about exactly these bytes being read back: the folded `Subject`,
/// the two recipients on one line, and the blank line that ends the headers are
/// each what one test is about.
const MESSAGE: &str = concat!(
    "From: Bob Builder <bob@example.com>\r\n",
    "To: Alice <alice@example.com>, carol@example.com\r\n",
    "Subject: A message that was\r\n",
    " already a message\r\n",
    "Message-ID: <first@example.com>\r\n",
    "MIME-Version: 1.0\r\n",
    "Content-Type: text/plain; charset=utf-8\r\n",
    "\r\n",
    "It arrived as bytes and it stays bytes.\r\n",
);

/// A server with an inbox, and the client to talk to it with.
struct Fixture {
    server: MockServer,
    client: Client,
    account_id: Id,
    inbox: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let inbox = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_mailbox("Inbox", Some(role::INBOX))
        };
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        Self {
            server,
            client,
            account_id,
            inbox,
        }
    }

    /// Upload `source` as a message blob and answer the blob id — the first half
    /// of every import, since a method call cannot carry bytes.
    fn upload(&self, source: &str) -> Id {
        self.client
            .upload_blob(
                &self.account_id,
                "message/rfc822",
                source.as_bytes().to_vec(),
            )
            .unwrap()
            .blob_id
    }

    /// The current `Email` state, for a before-and-after or a stale `ifInState`.
    fn email_state(&self) -> jmap_proto::State {
        self.client.email_state(&self.account_id).unwrap()
    }
}

#[test]
fn an_uploaded_message_becomes_an_email_in_the_mailbox() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    let imported = fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id.clone(), fixture.inbox.clone()).keyword(keyword::SEEN),
        )
        .unwrap();

    // The four properties RFC 8621 §4.8 has the server answer with.
    let id = imported.id.clone().expect("the server named the message");
    assert_eq!(imported.blob_id, Some(blob_id));
    assert!(imported.thread_id.is_some());
    assert_eq!(imported.size, Some(MESSAGE.len() as u64));

    // And it is in the mailbox it was filed into, with the keyword it was given.
    let fetched = fixture
        .client
        .email_get(
            &fixture.account_id,
            std::slice::from_ref(&id),
            Some(&["mailboxIds", "keywords", "size"]),
        )
        .unwrap();
    let email = &fetched[0];
    assert_eq!(email.id, Some(id));
    assert_eq!(
        email.mailbox_ids.as_ref().unwrap().get(&fixture.inbox),
        Some(&true)
    );
    assert_eq!(
        email.keywords.as_ref().unwrap().get(keyword::SEEN),
        Some(&true)
    );
    assert_eq!(email.size, Some(MESSAGE.len() as u64));
}

#[test]
fn the_headers_of_an_imported_message_become_its_row() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    let id = fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id, fixture.inbox.clone()),
        )
        .unwrap()
        .id
        .unwrap();

    let fetched = fixture
        .client
        .email_get(
            &fixture.account_id,
            &[id],
            Some(&["subject", "from", "to", "messageId", "preview"]),
        )
        .unwrap();
    let email = &fetched[0];

    // A folded header is one header (RFC 5322 §2.2.3): what the message list
    // shows is the unfolded value, on one line.
    assert_eq!(
        email.subject.as_deref(),
        Some("A message that was already a message")
    );

    let from = email.from.as_ref().expect("From was read");
    assert_eq!(from.len(), 1);
    assert_eq!(from[0].email, "bob@example.com");
    assert_eq!(from[0].name.as_deref(), Some("Bob Builder"));

    // Two recipients on one line, one of them without a display name.
    let to = email.to.as_ref().expect("To was read");
    assert_eq!(to.len(), 2);
    assert_eq!(to[0].email, "alice@example.com");
    assert_eq!(to[0].name.as_deref(), Some("Alice"));
    assert_eq!(to[1].email, "carol@example.com");
    assert_eq!(to[1].name, None);

    assert_eq!(
        email.message_id.as_deref(),
        Some(["first@example.com".to_owned()].as_slice())
    );
    assert_eq!(
        email.preview.as_deref(),
        Some("It arrived as bytes and it stays bytes.")
    );
}

#[test]
fn an_imported_message_downloads_as_the_bytes_it_went_up_as() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    let imported = fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id, fixture.inbox.clone()),
        )
        .unwrap();

    // The round trip that matters to a mail client: what it appended is what it
    // opens afterwards. A server that repaired the message would answer with a
    // blobId of its own — hence the download goes to the reported one, not the
    // uploaded one.
    let source = fixture
        .client
        .download_blob(
            &fixture.account_id,
            imported.blob_id.as_ref().unwrap(),
            "message.eml",
        )
        .unwrap();
    assert_eq!(String::from_utf8(source).unwrap(), MESSAGE);
}

#[test]
fn an_import_is_a_change_the_next_sync_is_told_about() {
    let fixture = Fixture::start();
    let before = fixture.email_state();
    let blob_id = fixture.upload(MESSAGE);

    let id = fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id, fixture.inbox.clone()),
        )
        .unwrap()
        .id
        .unwrap();

    // An import is a state transition, not a seeding: a client that syncs
    // incrementally has to hear about a message it imported itself, because the
    // same delta is what tells it about the ones another client imported.
    let changes = fixture
        .client
        .changes(&fixture.account_id, "Email", &before)
        .unwrap();
    assert_eq!(changes.created, vec![id]);
    assert!(changes.updated.is_empty());
    assert!(changes.destroyed.is_empty());
    assert_ne!(changes.new_state, before);
}

#[test]
fn an_import_sorts_by_the_date_it_is_given() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    let imported = fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id, fixture.inbox.clone()).received_at("2026-03-04T05:06:07Z"),
        )
        .unwrap();

    let fetched = fixture
        .client
        .email_get(
            &fixture.account_id,
            &[imported.id.unwrap()],
            Some(&["receivedAt"]),
        )
        .unwrap();
    assert_eq!(
        fetched[0].received_at,
        Some(UtcDate::new("2026-03-04T05:06:07Z"))
    );
}

#[test]
fn an_import_of_a_blob_that_was_never_uploaded_is_refused() {
    let fixture = Fixture::start();

    let result = fixture.client.email_import(
        &fixture.account_id,
        &EmailImport::new("B404", fixture.inbox.clone()),
    );

    // RFC 8621 §4.8: a blobId that is "missing, wrong type, id not found" is an
    // invalidProperties refusal of that one message.
    match result {
        Err(Error::Set(error)) => assert_eq!(error.error_type, set::INVALID_PROPERTIES),
        other => panic!("expected invalidProperties, got {other:?}"),
    }
    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert!(
        state
            .account(&fixture.account_id)
            .unwrap()
            .emails
            .is_empty()
    );
}

#[test]
fn an_import_into_no_mailbox_is_refused() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    // "At least one Mailbox MUST be given" — a map whose every value is false
    // names none, exactly as it does for an `Email/set`.
    let result = fixture.client.email_import(
        &fixture.account_id,
        &EmailImport {
            blob_id: Some(blob_id),
            mailbox_ids: Some([(fixture.inbox.clone(), false)].into()),
            ..EmailImport::default()
        },
    );
    match result {
        Err(Error::Set(error)) => assert_eq!(error.error_type, set::INVALID_PROPERTIES),
        other => panic!("expected invalidProperties, got {other:?}"),
    }
}

#[test]
fn an_import_into_a_mailbox_that_is_not_there_is_refused() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    let result = fixture
        .client
        .email_import(&fixture.account_id, &EmailImport::new(blob_id, "M404"));
    match result {
        Err(Error::Set(error)) => assert_eq!(error.error_type, set::INVALID_PROPERTIES),
        other => panic!("expected invalidProperties, got {other:?}"),
    }
}

#[test]
fn an_import_of_bytes_that_are_not_a_message_is_refused() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload("this was never a message");

    // RFC 8621 §4.8 allows a server either to repair such a blob or to refuse
    // it; refusing is what this one does, and the client has to be able to tell
    // that refusal from a mailbox it got wrong.
    let result = fixture.client.email_import(
        &fixture.account_id,
        &EmailImport::new(blob_id, fixture.inbox.clone()),
    );
    match result {
        Err(Error::Set(error)) => {
            assert_eq!(error.error_type, email_import_error::INVALID_EMAIL)
        }
        other => panic!("expected invalidEmail, got {other:?}"),
    }
    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert!(
        state
            .account(&fixture.account_id)
            .unwrap()
            .emails
            .is_empty()
    );
}

#[test]
fn one_import_refused_does_not_hold_up_another_in_the_same_call() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    // "Each Email to import is considered an atomic unit that may succeed or
    // fail individually" — which is only visible in a call carrying two.
    let request = EmailImportRequest::new(fixture.account_id.clone())
        .import("good", EmailImport::new(blob_id, fixture.inbox.clone()))
        .import("bad", EmailImport::new("B404", fixture.inbox.clone()));
    let arguments = fixture
        .client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Email/import",
            &request,
        )
        .unwrap();
    let response: jmap_proto::mail::EmailImportResponse =
        serde_json::from_value(arguments).unwrap();

    let created = response.created.expect("the good one landed");
    assert_eq!(created.len(), 1);
    assert!(created["good"].id.is_some());
    let not_created = response.not_created.expect("the bad one was refused");
    assert_eq!(not_created.len(), 1);
    assert_eq!(not_created["bad"].error_type, set::INVALID_PROPERTIES);
}

#[test]
fn an_import_against_a_state_that_has_moved_on_is_refused_whole() {
    let fixture = Fixture::start();
    let stale = fixture.email_state();
    let blob_id = fixture.upload(MESSAGE);

    // Another client imports first, so the state the caller holds is old.
    fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id.clone(), fixture.inbox.clone()),
        )
        .unwrap();

    let request = EmailImportRequest::new(fixture.account_id.clone())
        .import("second", EmailImport::new(blob_id, fixture.inbox.clone()))
        .if_in_state(stale);
    let result = fixture.client.single_call(
        &[CAPABILITY_CORE, CAPABILITY_MAIL],
        "Email/import",
        &request,
    );
    match result {
        Err(Error::Method(error)) => {
            assert_eq!(error.error_type, jmap_proto::error::method::STATE_MISMATCH)
        }
        other => panic!("expected stateMismatch, got {other:?}"),
    }

    // Aborted, not partially applied: still the one message from before.
    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert_eq!(state.account(&fixture.account_id).unwrap().emails.len(), 1);
}

#[test]
fn the_same_message_imported_twice_is_two_messages() {
    let fixture = Fixture::start();
    let blob_id = fixture.upload(MESSAGE);

    let first = fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id.clone(), fixture.inbox.clone()),
        )
        .unwrap();
    let second = fixture
        .client
        .email_import(
            &fixture.account_id,
            &EmailImport::new(blob_id, fixture.inbox.clone()),
        )
        .unwrap();

    // RFC 8621 §4.8 lets a server forbid two copies of one message with an
    // `alreadyExists`; this one takes the other branch, and then the two must be
    // separate objects with their own ids. Asserted so that turning the mock
    // into the strict kind is a decision someone makes on purpose.
    assert_ne!(first.id, second.id);
    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert_eq!(state.account(&fixture.account_id).unwrap().emails.len(), 2);
}
