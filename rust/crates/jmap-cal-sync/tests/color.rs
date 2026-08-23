// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::set_color` against the mock server: the whole of what a
//! calendar-colour write-back means, minus the vfunc/diff bookkeeping.

mod common;

use std::sync::{Arc, Mutex};

use common::Fixture;
use jmap_proto::Id;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

#[test]
fn set_color_reaches_the_server() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    sync.set_color(Some("#00ff00")).unwrap();

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color.as_deref(), Some("#00ff00"));
}

#[test]
fn set_color_of_none_clears_it() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    sync.set_color(Some("#00ff00")).unwrap();

    sync.set_color(None).unwrap();

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color, None);
}

/// Records every event this crate emits (level + fields), so a test can
/// assert a call attached structured fields rather than only a free-text
/// message — the same minimal harness `jmap-client::client`'s own tests use,
/// duplicated here because this crate depends on `tracing`, not
/// `tracing-subscriber`.
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

#[test]
fn set_color_traces_the_account_and_calendar_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        captured: captured.clone(),
    };

    tracing::subscriber::with_default(subscriber, || {
        sync.set_color(Some("#00ff00")).unwrap();
    });

    let captured = captured.lock().unwrap();
    assert!(
        captured
            .iter()
            .any(|(level, name, value)| *level == Level::DEBUG
                && name == "account_id"
                && *value == fixture.account_id.to_string()),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(level, name, value)| *level == Level::DEBUG
                && name == "calendar_id"
                && *value == fixture.ours.to_string()),
        "expected a DEBUG calendar_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful push should not log an error field, got {captured:?}"
    );
}

#[test]
fn set_color_traces_the_failure_when_the_calendar_is_gone() {
    let fixture = Fixture::start();
    let sync = jmap_cal_sync::CalSync::new(
        fixture.client(),
        fixture.account_id.clone(),
        Id::new("nonexistent"),
    );
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        captured: captured.clone(),
    };

    tracing::subscriber::with_default(subscriber, || {
        let _ = sync.set_color(Some("#00ff00"));
    });

    let captured = captured.lock().unwrap();
    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
