// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts that the race tests share: a gate that parks one response.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

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
