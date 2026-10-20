// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Capture this process's `tracing` events, one test at a time.
//!
//! This design avoids a race that a per-test subscriber installed with
//! `tracing::subscriber::with_default` hits when other tests run
//! concurrently on other threads.
//!
//! `tracing-core` registers a callsite's `Interest` (never, sometimes or
//! always) once per process. It computes the `Interest` the first time the
//! callsite fires, from the `Dispatch` current on the calling thread at
//! that instant. A `with_default` subscriber is current only on its own
//! thread. A test on another thread, with no subscriber of its own, hits
//! the default no-op dispatch. That dispatch caches `Interest::never` for
//! the first callsite it touches. The verdict is permanent. A later test
//! that captures the same callsite is silently skipped, before its
//! subscriber ever runs. Calling
//! `tracing::callsite::rebuild_interest_cache()` does not fix this: it
//! only re-evaluates callsites the registry already knows, not one no
//! thread has run yet.
//!
//! The fix: install one subscriber globally, once per process, with
//! `tracing::subscriber::set_global_default`. Its `enabled()` method
//! always returns `true`. Every callsite then resolves its first
//! registration to `Interest::always`, no matter which thread touches it
//! first, so the race above cannot happen.
//!
//! Routing gives each test its own events. The global subscriber forwards
//! an event only to the buffer the calling thread currently has scoped
//! through [`capture`]. Two tests that capture concurrently on different
//! threads never see each other's events, and no lock is needed between
//! them.
//!
//! Call [`capture`] around every call in a test binary that might reach a
//! traced callsite, even a call whose result a test does not inspect. The
//! global subscriber installs lazily, on a thread's first call to
//! `capture`, so a bare call outside `capture` could still be the first
//! touch of a callsite, racing the install on another thread.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// One captured event's level, field name and value. A string field keeps
/// its raw text; any other field is Debug-formatted.
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

/// Runs `run` and captures every `tracing` event it emits on the calling
/// thread. Returns `run`'s result together with the captured events.
///
/// Safe to call from several test threads in the same test binary at once:
/// see the module documentation for why. Call this for every traced call
/// a test makes, even one whose events the test does not inspect, and
/// discard the second element instead of calling the traced code directly.
/// This way, `capture` always installs the global subscriber before any
/// callsite on that thread fires first.
pub fn capture<T>(run: impl FnOnce() -> T) -> (T, Captured) {
    ensure_installed();
    let sink = Arc::new(Mutex::new(Vec::new()));
    let outer = SCOPE.with(|scope| scope.replace(Some(sink.clone())));
    let restore = Restore(outer);
    let result = run();
    drop(restore);
    let captured = std::mem::take(&mut *sink.lock().unwrap());
    (result, captured)
}

/// Restores the thread's scope to an enclosing `capture`, if one exists,
/// even when `run` panics.
struct Restore(Option<Arc<Mutex<Captured>>>);

impl Drop for Restore {
    fn drop(&mut self) {
        let outer = self.0.take();
        SCOPE.with(|scope| *scope.borrow_mut() = outer);
    }
}
