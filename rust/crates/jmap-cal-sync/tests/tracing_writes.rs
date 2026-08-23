// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::save_component`/`remove_component` trace their writes with
//! `account_id`/`calendar_id`, the next Track B1 slice after `set_color`
//! (`tests/color.rs`).

mod common;

use std::sync::{Arc, Mutex};

use common::Fixture;
use jmap_proto::Id;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

const NEW_EVENT: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:20260808T101500Z-4711-1000-1-0@localhost\r\n\
SUMMARY:Planning\r\n\
DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
DURATION:PT90M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

/// Records every event this crate emits (level + fields), duplicated from
/// `tests/color.rs` for the same reason: this crate depends on `tracing`,
/// not `tracing-subscriber`.
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

/// Serializes `capture()` within this test binary, and forces a fresh
/// callsite-interest rebuild once this subscriber is the thread's default.
/// Both guard against the same underlying fact: `tracing-core` caches each
/// macro call site's `Interest` (never/sometimes/always) *once*, process-
/// wide, the first time that call site fires, based on whichever
/// `Dispatch`es were alive at that moment — not on which `Dispatch` is
/// current on a given call. A cached decision from before this subscriber
/// existed (or from a concurrently-running test's own `capture()` racing
/// this one) can otherwise survive and apply to *this* call, which is how
/// this harness previously (a) panicked in `Arc::try_unwrap` on a
/// transiently-elevated strong count from another thread's `Dispatch`, and
/// (b) silently dropped one crate's own event while sibling call sites
/// (from `jmap-client`, already registered under a live subscriber deeper
/// in the same process) kept firing. `rebuild_interest_cache` re-evaluates
/// every known call site against whatever is current *right now*, so it
/// must run after `with_default` has installed this subscriber, not before.
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

/// Like `capture`, but for fixture setup that happens to call a traced
/// function and must not run unguarded (see `capture`'s own doc): an
/// unguarded call is this exact call site's *first-ever* invocation in the
/// process often enough to matter, which would otherwise cache its
/// `Interest` as "never" before any subscriber has had a say.
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

#[test]
fn creating_an_event_traces_the_account_and_calendar_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        sync.save_component(NEW_EVENT, None).unwrap();
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
            "calendar_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG calendar_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful create should not log an error field, got {captured:?}"
    );
}

#[test]
fn creating_an_event_in_a_gone_calendar_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = jmap_cal_sync::CalSync::new(
        fixture.client(),
        fixture.account_id.clone(),
        Id::new("nonexistent"),
    );

    let captured = capture(|| {
        let _ = sync.save_component(NEW_EVENT, None);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn updating_an_event_traces_the_account_calendar_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let saved = untraced(|| sync.save_component(NEW_EVENT, None).unwrap());
    let edited = saved
        .icalendar
        .replace("SUMMARY:Planning", "SUMMARY:Planning (moved)");

    let captured = capture(|| {
        sync.save_component(&edited, Some(&saved.uid)).unwrap();
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
            "calendar_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG calendar_id field, got {captured:?}"
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
fn removing_an_event_traces_the_account_calendar_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let saved = untraced(|| sync.save_component(NEW_EVENT, None).unwrap());

    let captured = capture(|| {
        sync.remove_component(&saved.uid).unwrap();
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
            "calendar_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG calendar_id field, got {captured:?}"
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
fn removing_a_nonexistent_event_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        let _ = sync.remove_component("nonexistent");
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
