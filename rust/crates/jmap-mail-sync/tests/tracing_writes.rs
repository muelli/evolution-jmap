// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::create_folder`/`delete_folder`, `set_subscribed`/
//! `rename_folder`, `set_keywords`/`file_message`, `expunge_message`, and
//! `import_message`/`send_message` trace their writes with `account_id` and
//! the folder's/message's name/id, the Track B1 slice after
//! `jmap-book-sync`'s (`tests/tracing_writes.rs` there).

use std::sync::{Arc, Mutex};

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{Filing, KeywordChange, Keywords, MailSync, Outgoing};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{Envelope, EnvelopeAddress};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        Self::started_with(MockServer::builder())
    }

    fn started_with(builder: jmap_mock::MockServerBuilder) -> Self {
        let server = builder.start();
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

    fn seed_identity(&self, name: &str, email: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_identity(name, email)
    }

    /// One message, filed in `mailbox`, with no keywords.
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

#[test]
fn setting_keywords_traces_the_account_and_uid_on_success() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();
    let before = Keywords::default();
    let after = Keywords::new(
        &jmap_mail_sync::MessageFlags {
            seen: true,
            ..Default::default()
        },
        &[],
    );
    let change = KeywordChange::between(&before, &after);

    let captured = capture(|| {
        sync.set_keywords(&uid, &change).unwrap();
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
        has(&captured, Level::DEBUG, "uid", uid.as_ref()),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful keyword change should not log an error field, got {captured:?}"
    );
}

#[test]
fn setting_keywords_on_a_nonexistent_message_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let before = Keywords::default();
    let after = Keywords::new(
        &jmap_mail_sync::MessageFlags {
            seen: true,
            ..Default::default()
        },
        &[],
    );
    let change = KeywordChange::between(&before, &after);

    let captured = capture(|| {
        let _ = sync.set_keywords(&Id::new("E404"), &change);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn setting_an_empty_keyword_change_traces_nothing_at_all() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();
    let same = Keywords::default();
    let change = KeywordChange::between(&same, &same);
    assert!(change.is_empty());

    let captured = capture(|| {
        sync.set_keywords(&uid, &change).unwrap();
    });

    assert!(
        captured.is_empty(),
        "an empty change sends no request and should trace nothing, got {captured:?}"
    );
}

#[test]
fn filing_a_message_traces_the_account_and_uid_on_success() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.file_message(&uid, &Filing::copied_into(archive.clone()))
            .unwrap();
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
        has(&captured, Level::DEBUG, "uid", uid.as_ref()),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful filing should not log an error field, got {captured:?}"
    );
}

#[test]
fn filing_a_nonexistent_message_traces_the_failure() {
    let fixture = Fixture::start();
    let archive = fixture.seed_mailbox("Archive");
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.file_message(&Id::new("E404"), &Filing::copied_into(archive));
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn filing_a_message_into_the_mailbox_it_is_already_in_traces_nothing_at_all() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();
    let filing = Filing::moved(inbox.clone(), inbox);
    assert!(filing.is_empty());

    let captured = capture(|| {
        sync.file_message(&uid, &filing).unwrap();
    });

    assert!(
        captured.is_empty(),
        "an empty filing sends no request and should trace nothing, got {captured:?}"
    );
}

#[test]
fn expunging_a_message_this_mailbox_is_the_last_home_of_traces_the_account_uid_and_mailbox() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.expunge_message(&uid, &inbox).unwrap();
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
        has(&captured, Level::DEBUG, "uid", uid.as_ref()),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "mailbox_id", inbox.as_ref()),
        "expected a DEBUG mailbox_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful expunge should not log an error field, got {captured:?}"
    );
}

#[test]
fn expunging_a_message_filed_elsewhere_too_traces_the_account_uid_and_mailbox() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&inbox);
    let sync = fixture.sync();
    sync.file_message(&uid, &Filing::copied_into(archive))
        .unwrap();

    let captured = capture(|| {
        sync.expunge_message(&uid, &inbox).unwrap();
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
        has(&captured, Level::DEBUG, "uid", uid.as_ref()),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "mailbox_id", inbox.as_ref()),
        "expected a DEBUG mailbox_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful expunge should not log an error field, got {captured:?}"
    );
}

#[test]
fn expunging_a_message_that_is_not_in_this_mailbox_traces_no_write() {
    // The read that decides this (`Email/get`) still happens and leaves its own
    // transport-level trace noise, unlike `set_keywords`/`file_message`'s
    // short-circuit, which sends no request at all — so this asserts the
    // absence of this crate's own write trace rather than an empty capture.
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let archive = fixture.seed_mailbox("Archive");
    let uid = fixture.seed_message(&archive);
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.expunge_message(&uid, &inbox).unwrap();
    });

    assert!(
        !has(&captured, Level::DEBUG, "mailbox_id", inbox.as_ref()),
        "a no-op expunge should not trace a write, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a no-op expunge should not log an error field, got {captured:?}"
    );
}

#[test]
fn expunging_a_nonexistent_message_traces_the_failure() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.expunge_message(&Id::new("E404"), &inbox);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

const MESSAGE: &[u8] = b"From: Bob <bob@example.com>\r\n\
To: Alice <alice@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

#[test]
fn importing_a_message_traces_the_account_and_mailbox_id_on_success() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
            .unwrap();
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
        has(&captured, Level::DEBUG, "mailbox_id", inbox.as_ref()),
        "expected a DEBUG mailbox_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful import should not log an error field, got {captured:?}"
    );
}

#[test]
fn importing_bytes_that_are_not_a_message_traces_the_failure() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.import_message(
            &inbox,
            b"\x00\x01 not a message at all".to_vec(),
            &Keywords::default(),
            None,
        );
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn importing_a_message_over_the_accounts_upload_limit_traces_the_failure() {
    let fixture = Fixture::started_with(MockServer::builder().size_upload(1024));
    let inbox = fixture.seed_mailbox("Inbox");
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.import_message(&inbox, vec![b'x'; 4096], &Keywords::default(), None);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn sending_a_message_traces_the_account_and_identity_on_success() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts");
    let identity = fixture.seed_identity("Alice", "alice@example.com");
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity: identity.clone(),
            envelope: Some(Envelope {
                mail_from: EnvelopeAddress::new("alice@example.com"),
                rcpt_to: vec![EnvelopeAddress::new("bob@example.com")],
            }),
            staging: drafts,
            destination: None,
        })
        .unwrap();
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
        has(&captured, Level::DEBUG, "identity_id", identity.as_ref()),
        "expected a DEBUG identity_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful send should not log an error field, got {captured:?}"
    );
}

#[test]
fn sending_a_message_through_no_identity_traces_the_failure() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts");
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.send_message(Outgoing {
            source: MESSAGE.to_vec(),
            identity: Id::new("I404"),
            envelope: Some(Envelope {
                mail_from: EnvelopeAddress::new("alice@example.com"),
                rcpt_to: vec![EnvelopeAddress::new("bob@example.com")],
            }),
            staging: drafts,
            destination: None,
        });
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
