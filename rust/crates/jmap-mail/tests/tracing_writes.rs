// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structured tracing tests for `jmap-mail` — Track B1 follow-up.
//!
//! Asserts structured fields attached to `tracing` events during mail store and
//! transport operations: folder listing, message listing, changes diffing,
//! message fetching, keyword updates, filing, expunging, importing, subscriptions,
//! folder management (create/delete/rename), sending, and service lifecycle.

use std::sync::{Arc, Mutex};

use eds_sys::CAMEL_STORE_FOLDER_INFO_REFRESH;
use jmap_client::{Client, Credentials};
use jmap_mail::store::JmapStore;
use jmap_mail::transport::JmapTransport;
use jmap_mail_sync::{Filing, FolderInfo, KeywordChange, Keywords, MailSync};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{Envelope, EnvelopeAddress, role};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

struct CapturingSubscriber {
    captured: Arc<Mutex<Vec<(Level, String, String)>>>,
}

struct Recorder<'a> {
    level: Level,
    sink: &'a Mutex<Vec<(Level, String, String)>>,
}

impl Visit for Recorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_owned()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }
}

impl Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> SpanId {
        SpanId::from_u64(1)
    }

    fn record(&self, _span: &SpanId, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &SpanId, _follows: &SpanId) {}

    fn event(&self, event: &Event<'_>) {
        event.record(&mut Recorder {
            level: *event.metadata().level(),
            sink: &self.captured,
        });
    }

    fn enter(&self, _span: &SpanId) {}

    fn exit(&self, _span: &SpanId) {}
}

static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

fn capture(run: impl FnOnce()) -> Vec<(Level, String, String)> {
    let _serialize = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        captured: captured.clone(),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        run();
    });
    std::mem::take(&mut *captured.lock().unwrap())
}

fn untraced<T>(run: impl FnOnce() -> T) -> T {
    let _serialize = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let subscriber = CapturingSubscriber {
        captured: Arc::new(Mutex::new(Vec::new())),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        run()
    })
}

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

struct Fixture {
    server: MockServer,
    inbox_id: Id,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let inbox_id = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.accounts.get_mut(&account_id).unwrap();
            account.seed_mailbox("Inbox", Some(role::INBOX))
        };
        Self {
            server,
            inbox_id,
            account_id,
        }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).expect("connected");
        MailSync::new(client, self.account_id.clone())
    }

    fn store(&self) -> Box<JmapStore> {
        let store = JmapStore::detached();
        store.store_connection(self.sync());
        store
    }

    fn transport(&self) -> Box<JmapTransport> {
        let transport = JmapTransport::detached();
        transport.install_connection(self.sync());
        transport
    }
}

#[test]
fn store_connection_lifecycle_traces_structured_fields() {
    let fixture = untraced(Fixture::start);
    let store = JmapStore::detached();

    let sync = untraced(|| fixture.sync());
    let logs = capture(|| {
        store.store_connection(sync);
    });
    assert!(
        logs.iter().any(|(l, n, v)| *l == Level::DEBUG
            && n == "message"
            && v == "storing mail connection in store"),
        "expected connection store event, got {logs:?}"
    );

    let logs = capture(|| {
        let dropped = store.drop_connection();
        assert!(dropped);
    });
    assert!(
        has(&logs, Level::DEBUG, "dropped", "true"),
        "expected dropped=true field, got {logs:?}"
    );
}

#[test]
fn folders_traces_structured_fields_on_success() {
    let fixture = untraced(Fixture::start);
    let store = fixture.store();

    let logs = capture(|| {
        let tree = store
            .folders(CAMEL_STORE_FOLDER_INFO_REFRESH)
            .expect("folders");
        assert!(!tree.is_empty());
    });

    assert!(
        has(
            &logs,
            Level::DEBUG,
            "flags",
            &CAMEL_STORE_FOLDER_INFO_REFRESH.to_string()
        ),
        "expected flags field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "state"),
        "expected state field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "count"),
        "expected count field, got {logs:?}"
    );
}

#[test]
fn messages_and_messages_since_trace_structured_fields() {
    let fixture = untraced(Fixture::start);
    let store = fixture.store();

    let logs = capture(|| {
        let (state, messages) = store.messages(&fixture.inbox_id).expect("messages");
        assert!(messages.is_empty());

        let _update = store
            .messages_since(&fixture.inbox_id, &state, 0)
            .expect("messages_since");
    });

    assert!(
        has(&logs, Level::DEBUG, "mailbox_id", fixture.inbox_id.as_str()),
        "expected mailbox_id field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "state"),
        "expected state field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "since"),
        "expected since field, got {logs:?}"
    );
}

#[test]
fn message_source_and_keywords_trace_structured_fields() {
    let fixture = untraced(Fixture::start);
    let message_uid = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.accounts.get_mut(&fixture.account_id).unwrap();
        account.seed_email(EmailSeed::new(
            fixture.inbox_id.clone(),
            ("Alice", "alice@example.com"),
            "Test",
            "body",
            "2026-01-01T09:00:00Z",
        ))
    };
    let store = fixture.store();

    let logs = capture(|| {
        let source = store.message_source(&message_uid).expect("message_source");
        assert!(!source.is_empty());

        let before = Keywords::default();
        let after = Keywords::from_iter(["$seen".to_string()]);
        let change = KeywordChange::between(&before, &after);
        store
            .set_keywords(&message_uid, &change)
            .expect("set_keywords");
    });

    assert!(
        has(&logs, Level::DEBUG, "uid", message_uid.as_str()),
        "expected uid field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "size"),
        "expected size field, got {logs:?}"
    );
}

#[test]
fn file_and_expunge_message_trace_structured_fields() {
    let fixture = untraced(Fixture::start);
    let archive_id = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.accounts.get_mut(&fixture.account_id).unwrap();
        account.seed_mailbox("Archive", Some(role::ARCHIVE))
    };
    let message_uid = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.accounts.get_mut(&fixture.account_id).unwrap();
        account.seed_email(EmailSeed::new(
            fixture.inbox_id.clone(),
            ("Bob", "bob@example.com"),
            "Move me",
            "body",
            "2026-01-01T09:00:00Z",
        ))
    };
    let store = fixture.store();

    let logs = capture(|| {
        let filing = Filing::moved(fixture.inbox_id.clone(), archive_id.clone());
        store
            .file_message(&message_uid, &filing)
            .expect("file_message");

        store
            .expunge_message(&message_uid, &archive_id)
            .expect("expunge_message");
    });

    assert!(
        has(&logs, Level::DEBUG, "uid", message_uid.as_str()),
        "expected uid field, got {logs:?}"
    );
    assert!(
        has(&logs, Level::DEBUG, "mailbox_id", archive_id.as_str()),
        "expected mailbox_id field, got {logs:?}"
    );
}

#[test]
fn import_message_and_subscription_trace_structured_fields() {
    let fixture = untraced(Fixture::start);
    let store = fixture.store();

    let logs = capture(|| {
        let raw = b"From: me@example.com\r\nTo: you@example.com\r\nSubject: Import\r\n\r\nImported body\r\n".to_vec();
        let uid = store
            .import_message(&fixture.inbox_id, raw, &Keywords::default(), None)
            .expect("import_message");
        assert!(!uid.as_str().is_empty());

        store
            .set_subscribed(&fixture.inbox_id, true)
            .expect("set_subscribed");
    });

    assert!(
        has(&logs, Level::DEBUG, "mailbox_id", fixture.inbox_id.as_str()),
        "expected mailbox_id field, got {logs:?}"
    );
    assert!(
        has(&logs, Level::DEBUG, "subscribed", "true"),
        "expected subscribed=true field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "uid"),
        "expected uid field, got {logs:?}"
    );
}

#[test]
fn folder_management_traces_structured_fields() {
    let fixture = untraced(Fixture::start);
    let store = fixture.store();

    let (created_id, created_path) = {
        let mut id = Id::from("");
        let mut path = String::new();
        let logs = capture(|| {
            let created = store
                .create_folder(None, "Projects")
                .expect("create_folder");
            id = created.id;
            path = created.path;
        });

        assert!(
            has(&logs, Level::DEBUG, "name", "Projects"),
            "expected name=Projects field, got {logs:?}"
        );
        assert!(
            has(&logs, Level::DEBUG, "folder_id", id.as_str()),
            "expected folder_id field, got {logs:?}"
        );
        (id, path)
    };

    let folder_info = FolderInfo {
        id: created_id.clone(),
        path: created_path,
        display_name: "Projects".to_string(),
        role: None,
        total: 0,
        unread: 0,
        subscribed: false,
        children: Vec::new(),
    };

    let logs = capture(|| {
        let renamed = store
            .rename_folder(&folder_info, None, "Archived Projects")
            .expect("rename_folder");
        assert_eq!(renamed.display_name, "Archived Projects");

        store.delete_folder(&created_id).expect("delete_folder");
    });

    assert!(
        has(&logs, Level::DEBUG, "mailbox_id", created_id.as_str()),
        "expected mailbox_id field, got {logs:?}"
    );
    assert!(
        has(&logs, Level::DEBUG, "name", "Archived Projects"),
        "expected name field, got {logs:?}"
    );
}

#[test]
fn transport_send_message_traces_structured_fields() {
    let fixture = untraced(Fixture::start);
    {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.accounts.get_mut(&fixture.account_id).unwrap();
        account.seed_mailbox("Drafts", Some(role::DRAFTS));
        account.seed_mailbox("Sent", Some(role::SENT));
        account.seed_identity("Alice", "alice@example.com");
    }
    let transport = fixture.transport();

    let raw = b"From: Alice <alice@example.com>\r\nTo: Bob <bob@example.com>\r\nSubject: Hi\r\n\r\nSent body\r\n".to_vec();
    let envelope = Envelope {
        mail_from: EnvelopeAddress::new("alice@example.com"),
        rcpt_to: vec![EnvelopeAddress::new("bob@example.com")],
    };

    let logs = capture(|| {
        let sent = transport.send_message(raw, envelope).expect("send_message");
        assert!(!sent.uid.as_str().is_empty());
    });

    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "size"),
        "expected size field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "uid"),
        "expected uid field, got {logs:?}"
    );
    assert!(
        has(&logs, Level::DEBUG, "saved", "true"),
        "expected saved=true field, got {logs:?}"
    );
}

#[test]
fn store_operations_error_paths_trace_structured_fields() {
    let fixture = untraced(Fixture::start);
    let store = fixture.store();

    let logs = capture(|| {
        let _ = store.delete_folder(&Id::from("nonexistent_mailbox"));
    });

    assert!(
        has(&logs, Level::DEBUG, "mailbox_id", "nonexistent_mailbox"),
        "expected mailbox_id field, got {logs:?}"
    );
    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "failure"),
        "expected failure field, got {logs:?}"
    );
}

#[test]
fn send_message_identity_error_traces_failure_field() {
    let fixture = untraced(Fixture::start);
    let transport = fixture.transport();

    let raw = b"From: Unknown <unknown@example.com>\r\nTo: Bob <bob@example.com>\r\n\r\nBody\r\n"
        .to_vec();
    let envelope = Envelope {
        mail_from: EnvelopeAddress::new("unknown@example.com"),
        rcpt_to: vec![EnvelopeAddress::new("bob@example.com")],
    };

    let logs = capture(|| {
        let result = transport.send_message(raw, envelope);
        assert!(result.is_err());
    });

    assert!(
        logs.iter()
            .any(|(l, n, _)| *l == Level::DEBUG && n == "failure"),
        "expected failure field, got {logs:?}"
    );
}
