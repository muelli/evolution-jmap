// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `create_collection`/`delete_collection` trace their writes with
//! `account_id`/`kind`, the last Track B1 slice named as still open —
//! `jmap-cal-sync`, `jmap-book-sync`, and `jmap-mail-sync` all got theirs
//! first.

use std::sync::{Arc, Mutex};

use jmap_client::{Client, Credentials};
use jmap_collection_sync::{ChildKind, Doomed, Requested, create_collection, delete_collection};
use jmap_mock::{DEFAULT_ACCOUNT_ID, MockServer};
use jmap_proto::Id;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Records every event this crate emits (level + fields), duplicated from
/// the sibling sync crates' own `tracing_writes.rs` for the same reason:
/// this crate depends on `tracing`, not `tracing-subscriber`.
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

fn client(server: &MockServer) -> Client {
    Client::connect(server.origin(), Credentials::none())
        .expect("the mock serves a session document")
}

#[test]
fn creating_an_address_book_traces_the_account_and_kind_on_success() {
    let server = MockServer::builder().start();

    let captured = capture(|| {
        create_collection(
            &client(&server),
            &Requested {
                kind: ChildKind::AddressBook,
                display_name: "Work".to_owned(),
            },
        )
        .expect("the mock creates address books");
    });

    assert!(
        has(&captured, Level::DEBUG, "account_id", DEFAULT_ACCOUNT_ID),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful create should not log an error field, got {captured:?}"
    );
}

#[test]
fn creating_a_collection_in_an_account_that_serves_none_traces_the_failure() {
    let server = MockServer::builder()
        .without_capability(jmap_proto::session::CAPABILITY_CONTACTS)
        .start();

    let captured = capture(|| {
        let _ = create_collection(
            &client(&server),
            &Requested {
                kind: ChildKind::AddressBook,
                display_name: "Work".to_owned(),
            },
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
fn deleting_a_calendar_traces_the_account_and_collection_id_on_success() {
    let server = MockServer::builder().start();
    let created = untraced(|| {
        create_collection(
            &client(&server),
            &Requested {
                kind: ChildKind::Calendar,
                display_name: "Trips".to_owned(),
            },
        )
        .expect("the mock creates calendars")
    });

    let captured = capture(|| {
        delete_collection(
            &client(&server),
            &Doomed {
                kind: ChildKind::Calendar,
                collection_id: created.collection_id.clone(),
            },
        )
        .expect("the mock destroys calendars");
    });

    assert!(
        has(&captured, Level::DEBUG, "account_id", DEFAULT_ACCOUNT_ID),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "collection_id",
            created.collection_id.as_ref()
        ),
        "expected a DEBUG collection_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful destroy should not log an error field, got {captured:?}"
    );
}

#[test]
fn deleting_a_collection_the_server_does_not_hold_traces_the_failure() {
    let server = MockServer::builder().start();

    let captured = capture(|| {
        let _ = delete_collection(
            &client(&server),
            &Doomed {
                kind: ChildKind::AddressBook,
                collection_id: Id::new("no-such-address-book"),
            },
        );
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
