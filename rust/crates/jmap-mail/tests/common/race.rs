// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts that the race tests share: a gate that parks one response, a
//! transport that parks it, and a store pointer for the racing threads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_mail::store::JmapStore;

/// The longest time a `Gate` wait blocks. A missed signal fails the test and does not hang it.
pub const GATE_TIMEOUT: Duration = Duration::from_secs(60);

/// Lets the test park the racing request's response, and learn when it has
/// actually parked, instead of racing a sleep against it.
#[derive(Default)]
pub struct Gate {
    started: Mutex<bool>,
    started_cond: Condvar,
    released: Mutex<bool>,
    released_cond: Condvar,
}

impl Gate {
    pub fn wait_for_start(&self) {
        let started = self.started.lock().unwrap();
        let (_guard, wait) = self
            .started_cond
            .wait_timeout_while(started, GATE_TIMEOUT, |started| !*started)
            .unwrap();
        assert!(
            !wait.timed_out(),
            "the gated response did not arrive in {GATE_TIMEOUT:?}"
        );
    }

    pub fn signal_start(&self) {
        *self.started.lock().unwrap() = true;
        self.started_cond.notify_all();
    }

    pub fn wait_for_release(&self) {
        let released = self.released.lock().unwrap();
        let (_guard, wait) = self
            .released_cond
            .wait_timeout_while(released, GATE_TIMEOUT, |released| !*released)
            .unwrap();
        assert!(
            !wait.timed_out(),
            "the test did not release the gate in {GATE_TIMEOUT:?}"
        );
    }

    pub fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.released_cond.notify_all();
    }
}

/// Forwards every request to a real transport. Holds back the response to
/// the first request whose body names `method`, until the test releases the
/// gate. The real server answers that request before the hold, so the answer
/// is the server's honest one for the state at the time of the request.
///
/// The match is on the request body, not on a call index. Other calls on the
/// same client can come before the race starts. A call counter could arm on
/// one of them and block that call on a gate that nothing releases.
pub struct DelayedDeliveryTransport {
    inner: UreqTransport,
    gate: Arc<Gate>,
    method: &'static str,
    delayed_once: AtomicBool,
}

impl DelayedDeliveryTransport {
    pub fn new(gate: Arc<Gate>, method: &'static str) -> Self {
        Self {
            inner: UreqTransport::default(),
            gate,
            method,
            delayed_once: AtomicBool::new(false),
        }
    }
}

impl Transport for DelayedDeliveryTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let is_gated_call = request
            .body
            .map(|body| String::from_utf8_lossy(body).contains(self.method))
            .unwrap_or(false);
        let response = self.inner.execute(request)?;
        if is_gated_call && !self.delayed_once.swap(true, Ordering::SeqCst) {
            self.gate.signal_start();
            self.gate.wait_for_release();
        }
        Ok(response)
    }
}

/// A raw pointer to a store, for the threads of a race test.
///
/// `JmapStore::detached()` is not a GObject, so no `Sync` proof lets the threads
/// share a `&JmapStore`. Camel's own vfunc dispatch is in the same situation: it
/// reaches one instance from several threads through a raw pointer.
#[derive(Clone, Copy)]
pub struct StorePtr(*const JmapStore);

// SAFETY: `StorePtr::new` makes its caller keep the store alive until the test
// joins every thread that holds the pointer.
unsafe impl Send for StorePtr {}

impl StorePtr {
    /// # Safety
    ///
    /// `store` must stay alive until the test joins every thread that holds the
    /// returned pointer.
    pub unsafe fn new(store: &JmapStore) -> Self {
        Self(store)
    }

    /// Takes `self` by value instead of reading `.0` at the call site. A
    /// closure that captures this value captures the whole `Send`
    /// wrapper. Otherwise, Rust's disjoint-capture rules would capture
    /// the bare pointer field alone, which is not `Send`.
    pub fn borrow<'a>(self) -> &'a JmapStore {
        // SAFETY: the contract of `new` keeps the store alive for every use.
        unsafe { &*self.0 }
    }
}
