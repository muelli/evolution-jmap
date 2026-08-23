// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::create_folder`/`delete_folder` and `set_subscribed`/
//! `rename_folder` trace their writes with `account_id` and the folder's
//! name/id, the Track B1 slice after `jmap-book-sync`'s
//! (`tests/tracing_writes.rs` there).

use std::sync::{Arc, Mutex};

use jmap_client::{Client, Credentials};
use jmap_mail_sync::MailSync;
use jmap_mock::MockServer;
use jmap_proto::Id;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

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
}

/// Records every event this crate emits (level + fields), duplicated from
/// `jmap-book-sync/tests/tracing_writes.rs` for the same reason: this crate
/// depends on `tracing`, not `tracing-subscriber`.
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

fn capture(run: impl FnOnce()) -> Vec<(Level, String, String)> {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        captured: captured.clone(),
    };
    tracing::subscriber::with_default(subscriber, run);
    Arc::try_unwrap(captured).unwrap().into_inner().unwrap()
}

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

#[test]
fn creating_a_folder_traces_the_account_and_name_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.create_folder(None, "Projects").unwrap();
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.account_id.as_ref()
        ),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "name", "Projects"),
        "expected a DEBUG name field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful create should not log an error field, got {captured:?}"
    );
}

#[test]
fn creating_a_folder_with_a_name_a_sibling_already_has_traces_the_failure() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Projects");
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.create_folder(None, "Projects");
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn deleting_a_folder_traces_the_account_and_mailbox_id_on_success() {
    let fixture = Fixture::start();
    let doomed = fixture.seed_mailbox("Projects");
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.delete_folder(&doomed).unwrap();
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.account_id.as_ref()
        ),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "mailbox_id", doomed.as_ref()),
        "expected a DEBUG mailbox_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful delete should not log an error field, got {captured:?}"
    );
}

#[test]
fn deleting_a_nonexistent_folder_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.delete_folder(&Id::new("M404"));
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn subscribing_a_folder_traces_the_account_and_mailbox_id_on_success() {
    let fixture = Fixture::start();
    let mailbox = fixture.seed_mailbox("Projects");
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.set_subscribed(&mailbox, true).unwrap();
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.account_id.as_ref()
        ),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "mailbox_id", mailbox.as_ref()),
        "expected a DEBUG mailbox_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful subscription change should not log an error field, got {captured:?}"
    );
}

#[test]
fn subscribing_a_nonexistent_folder_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.set_subscribed(&Id::new("M404"), true);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn renaming_a_folder_traces_the_account_and_mailbox_id_on_success() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Projects");
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.rename_folder(&folder, None, "Archive").unwrap();
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.account_id.as_ref()
        ),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "mailbox_id", folder.as_ref()),
        "expected a DEBUG mailbox_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful rename should not log an error field, got {captured:?}"
    );
}

#[test]
fn renaming_a_nonexistent_folder_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.rename_folder(&Id::new("M404"), None, "Archive");
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
