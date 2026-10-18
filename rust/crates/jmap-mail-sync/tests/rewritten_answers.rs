// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Answers the mock server never gives on its own: a failure that is not
//! `cannotCalculateChanges`, and the races where another client destroys a
//! message between two calls of ours.
//!
//! The mock keeps its store consistent, so these answers are made by
//! rewriting its real response on the way back to the client. Each test
//! rewrites one method's answer and leaves every other call untouched.

use std::sync::Arc;
use std::time::Duration;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, MessageUpdate, SyncError};
use jmap_mock::{AccountState, EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;
use serde_json::{Value, json};

type Rewrite = Arc<dyn Fn(&mut Value) + Send + Sync>;

/// The real transport, with `rewrite` applied to every JSON body it answers
/// with: the session document and every API response.
struct Rewriting {
    inner: UreqTransport,
    rewrite: Rewrite,
}

impl Transport for Rewriting {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let mut response = self.inner.execute(request)?;
        if let Ok(mut body) = serde_json::from_slice::<Value>(&response.body) {
            (self.rewrite)(&mut body);
            response.body = serde_json::to_vec(&body).expect("JSON serialises");
        }
        Ok(response)
    }
}

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

    /// A sync whose client sees no rewritten answers.
    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// A sync whose client sees every answer to `method` (the invocation
    /// `[name, arguments, callId]`) through `rewrite`.
    fn sync_rewriting(
        &self,
        method: &'static str,
        rewrite: impl Fn(&mut Value) + Send + Sync + 'static,
    ) -> MailSync {
        self.sync_rewriting_bodies(move |body: &mut Value| {
            let invocations = body
                .get_mut("methodResponses")
                .and_then(Value::as_array_mut);
            for invocation in invocations.into_iter().flatten() {
                if invocation[0] == method {
                    rewrite(invocation);
                }
            }
        })
    }

    /// A sync whose client sees every JSON body through `rewrite`.
    fn sync_rewriting_bodies(
        &self,
        rewrite: impl Fn(&mut Value) + Send + Sync + 'static,
    ) -> MailSync {
        let rewrite: Rewrite = Arc::new(rewrite);
        let client = Client::builder()
            .transport(Rewriting {
                inner: UreqTransport::new(Duration::from_secs(10)),
                rewrite,
            })
            .connect(self.server.origin(), Credentials::none())
            .unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    fn edit<R>(&self, edit: impl FnOnce(&mut AccountState) -> R) -> R {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        edit(state.account_mut(&self.account_id).unwrap())
    }

    fn message(mailbox: &Id, subject: &str) -> EmailSeed {
        EmailSeed::new(
            mailbox.clone(),
            ("Bob", "bob@example.com"),
            subject,
            "text",
            "2026-01-15T09:00:00Z",
        )
    }

    /// A message that predates every state a test asks from.
    fn seed(&self, mailbox: &Id, subject: &str) -> Id {
        self.edit(|account| account.seed_email(Self::message(mailbox, subject)))
    }

    /// A message that arrives as a state change `Email/changes` reports.
    fn deliver(&self, mailbox: &Id, subject: &str) -> Id {
        self.edit(|account| account.deliver_email(Self::message(mailbox, subject)))
    }
}

/// Replaces a method response with a method-level error of `kind`.
fn method_error(kind: &'static str) -> impl Fn(&mut Value) + Send + Sync {
    move |invocation: &mut Value| {
        let call_id = invocation[2].clone();
        *invocation = json!(["error", { "type": kind }, call_id]);
    }
}

/// Turns every `destroyed` id of an `Email/set` answer into a `notDestroyed`
/// entry of `kind`.
fn not_destroyed(kind: &'static str) -> impl Fn(&mut Value) + Send + Sync {
    move |invocation: &mut Value| {
        let arguments = &mut invocation[1];
        let destroyed = arguments["destroyed"].take();
        let refused: serde_json::Map<String, Value> = destroyed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|id| (id.to_string(), json!({ "type": kind })))
            .collect();
        arguments["destroyed"] = json!([]);
        arguments["notDestroyed"] = Value::Object(refused);
    }
}

/// Only `cannotCalculateChanges` and an unrecognised state mean "list the
/// folders again". Any other failure of `Mailbox/changes` is a failure, and a
/// rebuilt tree would hide it.
#[test]
fn an_ordinary_failure_of_mailbox_changes_is_reported_not_rebuilt_over() {
    let fixture = Fixture::start();
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let (state, _) = fixture.sync().folder_tree().unwrap();

    let result = fixture
        .sync_rewriting("Mailbox/changes", method_error("serverFail"))
        .folder_tree_since(&state);

    assert!(
        matches!(result, Err(SyncError::Client(_))),
        "a serverFail must fail the refresh, got {result:?}"
    );
}

/// The same rule for a mailbox's messages: a failure of `Email/changes` that
/// is not about the state fails the refresh instead of listing the mailbox.
#[test]
fn an_ordinary_failure_of_email_changes_is_reported_not_relisted_over() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    fixture.seed(&inbox, "Old");
    let (state, held) = fixture.sync().messages(&inbox).unwrap();

    let result = fixture
        .sync_rewriting("Email/changes", method_error("serverFail"))
        .messages_since(&inbox, &state, held.len());

    assert!(
        matches!(result, Err(SyncError::Client(_))),
        "a serverFail must fail the refresh, got {result:?}"
    );
}

/// A message the delta names and the `Email/get` after it does not answer for
/// was destroyed between the two calls. The folder may hold a row for it, so
/// it must be reported absent, not dropped from the answer.
#[test]
fn a_message_destroyed_between_the_delta_and_its_fetch_is_reported_absent() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    fixture.seed(&inbox, "Old");
    let (state, held) = fixture.sync().messages(&inbox).unwrap();
    let vanished = fixture.deliver(&inbox, "Gone before it was fetched");

    let update = fixture
        .sync_rewriting("Email/get", |invocation| {
            let arguments = &mut invocation[1];
            let listed = arguments["list"].take();
            let ids: Vec<Value> = listed
                .as_array()
                .into_iter()
                .flatten()
                .map(|email| email["id"].clone())
                .collect();
            arguments["list"] = json!([]);
            arguments["notFound"] = Value::Array(ids);
        })
        .messages_since(&inbox, &state, held.len())
        .unwrap();

    match update {
        MessageUpdate::Changed {
            present, absent, ..
        } => {
            assert!(present.is_empty(), "nothing was fetched: {present:?}");
            assert_eq!(absent, vec![vanished]);
        }
        other => panic!("expected a delta, got {other:?}"),
    }
}

/// Another client destroyed the message after our read of its mailboxes and
/// before our destroy. The server's `notFound` is the same answer as a uid
/// the account never held.
#[test]
fn a_message_destroyed_before_its_expunge_lands_is_no_such_message() {
    let fixture = Fixture::start();
    let trash = fixture.edit(|account| account.seed_mailbox("Trash", Some(role::TRASH)));
    let uid = fixture.seed(&trash, "Deleted twice");

    let result = fixture
        .sync_rewriting("Email/set", not_destroyed("notFound"))
        .expunge_message(&uid, &trash);

    match result {
        Err(SyncError::NoSuchMessage(missing)) => assert_eq!(missing, uid),
        other => panic!("expected NoSuchMessage, got {other:?}"),
    }
}

/// Any other refusal of the destroy is a failure the user must see. Reading
/// it as "already gone" would tell Camel the message left the folder while
/// the server still holds it.
#[test]
fn a_refused_expunge_is_reported_as_the_refusal() {
    let fixture = Fixture::start();
    let trash = fixture.edit(|account| account.seed_mailbox("Trash", Some(role::TRASH)));
    let uid = fixture.seed(&trash, "Not yours to delete");

    let result = fixture
        .sync_rewriting("Email/set", not_destroyed("forbidden"))
        .expunge_message(&uid, &trash);

    match result {
        Err(SyncError::Client(jmap_client::Error::Set(refusal))) => {
            assert_eq!(refusal.error_type, "forbidden");
        }
        other => panic!("expected the forbidden refusal, got {other:?}"),
    }
}

/// RFC 8620 §2 types `maxObjectsInGet` as an `UnsignedInt`, so 0 parses, but
/// a server that advertises 0 has said nothing usable. It gets the same
/// fallback as a server that leaves the limit out, not a fetch in chunks of
/// nothing.
#[test]
fn an_advertised_get_limit_of_zero_falls_back_rather_than_fetching_nothing() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let first = fixture.seed(&inbox, "First");
    let second = fixture.seed(&inbox, "Second");

    let sync = fixture.sync_rewriting_bodies(|body| {
        if let Some(core) = body
            .get_mut("capabilities")
            .and_then(|capabilities| capabilities.get_mut("urn:ietf:params:jmap:core"))
        {
            core["maxObjectsInGet"] = json!(0);
        }
    });
    let (_, messages) = sync.messages(&inbox).unwrap();

    let mut uids: Vec<Id> = messages.into_iter().map(|message| message.uid).collect();
    uids.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(uids, expected);
}
