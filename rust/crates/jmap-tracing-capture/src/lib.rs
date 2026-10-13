// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Capture this process's `tracing` events on a per-test basis, without the
//! callsite-interest race that a per-test `tracing::subscriber::with_default`
//! subscriber hits when other tests run concurrently on other threads.
//!
//! `tracing-core` registers a callsite's `Interest` (never/sometimes/always)
//! only once per process, the first time that callsite fires, computed from
//! whichever `Dispatch` is current *on the calling thread* at that instant.
//! A per-test subscriber installed with `with_default` is only current on
//! its own thread; a concurrently running test on another thread, with no
//! subscriber of its own, hits the default no-op dispatch, which caches
//! `Interest::never` for any callsite it touches first. That verdict is
//! permanent: a later capturing test that reaches the same callsite is
//! silently skipped before its subscriber is ever consulted, even if it
//! just called `tracing::callsite::rebuild_interest_cache()` (which only
//! re-evaluates callsites already known to the registry, not one no thread
//! has executed yet).
//!
//! The fix is a single subscriber installed globally
//! (`tracing::subscriber::set_global_default`), once per process, whose
//! `enabled()` always returns `true`. Every callsite's first-ever
//! registration then resolves to `Interest::always` regardless of which
//! thread touches it first, so the race above cannot happen. Per-test
//! isolation comes from routing: the global subscriber only forwards an
//! event to the buffer the *calling thread* currently has scoped via
//! [`capture`], so two tests capturing concurrently on different threads
//! never see each other's events, and no locking is needed.
//!
//! Every call in a test binary that might reach a traced callsite must go
//! through [`capture`], even a test that does not inspect the result:
//! installing the global subscriber happens lazily, on a thread's first
//! call to `capture`, so a bare call elsewhere could still be the very
//! first touch of a callsite, racing the install on another thread.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// One captured event's level, field name and debug-formatted value.
pub type Captured = Vec<(Level, String, String)>;

struct Recorder<'a> {
    level: Level,
    sink: &'a Mutex<Captured>,
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

thread_local! {
    static SCOPE: RefCell<Option<Arc<Mutex<Captured>>>> = const { RefCell::new(None) };
}

struct Router;

impl Subscriber for Router {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> SpanId {
        SpanId::from_u64(1)
    }

    fn record(&self, _span: &SpanId, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &SpanId, _follows: &SpanId) {}

    fn event(&self, event: &Event<'_>) {
        SCOPE.with(|scope| {
            if let Some(sink) = scope.borrow().as_ref() {
                event.record(&mut Recorder {
                    level: *event.metadata().level(),
                    sink,
                });
            }
        });
    }

    fn enter(&self, _span: &SpanId) {}

    fn exit(&self, _span: &SpanId) {}
}

fn ensure_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        tracing::subscriber::set_global_default(Router)
            .expect("jmap_tracing_capture::capture: a global subscriber is already set");
    });
}

/// Runs `run`, routing every `tracing` event it emits (on the calling
/// thread) into the returned buffer, and returns `run`'s own result
/// alongside it.
///
/// Safe to call concurrently from multiple test threads in the same test
/// binary: see the module documentation for why. A test that does not
/// inspect the captured events still needs to call this (and may discard
/// the second element) rather than invoking the traced code directly, to
/// keep the global subscriber's install a true first event in the process.
pub fn capture<T>(run: impl FnOnce() -> T) -> (T, Captured) {
    ensure_installed();
    let sink = Arc::new(Mutex::new(Vec::new()));
    SCOPE.with(|scope| *scope.borrow_mut() = Some(sink.clone()));
    let result = run();
    SCOPE.with(|scope| *scope.borrow_mut() = None);
    let captured = std::mem::take(&mut *sink.lock().unwrap());
    (result, captured)
}
