// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact`/`remove_contact` trace their writes with
//! `account_id`/`address_book_id`, the Track B1 slice after
//! `jmap-cal-sync`'s (`tests/tracing_writes.rs` there).

mod common;

use std::sync::{Arc, Mutex};

use common::Fixture;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

const NEW_CONTACT: &str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
UID:pas-id-68A2F1C400000000\r\n\
FN:Vera Oldenburg\r\n\
N:Oldenburg;Vera;;;\r\n\
EMAIL;TYPE=WORK:vera@example.com\r\n\
END:VCARD\r\n";

/// Records every event this crate emits (level + fields), duplicated from
/// `jmap-cal-sync/tests/tracing_writes.rs` for the same reason: this crate
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
fn creating_a_contact_traces_the_account_and_address_book_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.save_contact(NEW_CONTACT, None).unwrap();
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
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG address_book_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful create should not log an error field, got {captured:?}"
    );
}

#[test]
fn creating_a_contact_in_a_gone_address_book_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = jmap_book_sync::BookSync::new(
        fixture.client(),
        fixture.account_id.clone(),
        "nonexistent".into(),
    );

    let captured = capture(|| {
        let _ = sync.save_contact(NEW_CONTACT, None);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn updating_a_contact_traces_the_account_address_book_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let saved = sync.save_contact(NEW_CONTACT, None).unwrap();
    let edited = saved
        .vcard
        .replace("FN:Vera Oldenburg", "FN:Vera Oldenburg-Nord");

    let captured = capture(|| {
        sync.save_contact(&edited, Some(&saved.uid)).unwrap();
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
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG address_book_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "uid", &saved.uid),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful update should not log an error field, got {captured:?}"
    );
}

#[test]
fn removing_a_contact_traces_the_account_address_book_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let saved = sync.save_contact(NEW_CONTACT, None).unwrap();

    let captured = capture(|| {
        sync.remove_contact(&saved.uid).unwrap();
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
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG address_book_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "uid", &saved.uid),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful destroy should not log an error field, got {captured:?}"
    );
}

#[test]
fn removing_a_nonexistent_contact_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.remove_contact("nonexistent");
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
