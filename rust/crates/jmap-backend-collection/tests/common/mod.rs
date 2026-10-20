// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared test utilities and time-limit helpers for the `jmap-backend-collection`
//! tests.
//!
//! The EDS tests run in headless VMs. Some VMs have no user session bus, and the
//! daemon state can change. A wait without a limit on D-Bus, a mock server or a
//! synchronisation primitive can stop a test binary permanently at 0% CPU.
//!
//! `with_timeout` and `with_timeout_duration` run a test closure on a separate
//! worker thread with a hard time limit (default: 10 s). If an operation stops,
//! the test fails quickly with a clear panic. The test runner does not stop.

#![allow(dead_code)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// The default hard time limit for tests in this crate. Normal test operations
/// complete in less than 0.1 s. An operation that takes more than 10 s is stuck.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `f` on a separate thread with the default hard time limit
/// ([`TEST_TIMEOUT`]).
///
/// If `f` completes in time, this function returns its result. If `f` panics,
/// this function also panics. If `f` takes too long, this function panics
/// immediately with a timeout failure.
pub fn with_timeout<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    with_timeout_duration(TEST_TIMEOUT, f)
}

/// Runs `f` on a separate thread with the time limit `timeout`.
pub fn with_timeout_duration<R: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> R + Send + 'static,
) -> R {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let res = f();
        let _ = tx.send(res);
    });

    match rx.recv_timeout(timeout) {
        Ok(res) => {
            handle.join().expect("test thread panicked");
            res
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("test timed out after {timeout:?}; failing fast rather than blocking forever");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            handle.join().expect("test thread panicked");
            unreachable!();
        }
    }
}
