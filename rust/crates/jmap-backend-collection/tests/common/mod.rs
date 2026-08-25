// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared test utilities and bounded-execution helpers for `jmap-backend-collection` tests.
//!
//! EDS-gated tests run in headless VM environments without a user session bus or with
//! variable daemon states. Unbounded waits on D-Bus, mock servers, or synchronisation
//! primitives can wedge a test binary at 0% CPU forever.
//!
//! `with_timeout` and `with_timeout_duration` execute test closures on a dedicated worker
//! thread bounded by a hard deadline (default: 10s). If an operation wedges, it fails
//! fast with a descriptive panic instead of blocking the entire test runner.

#![allow(dead_code)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// The default hard deadline for tests in this crate.
/// Normal test operations complete in < 0.1s; anything exceeding 10s is wedged.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `f` on a dedicated thread with the default hard timeout ([`TEST_TIMEOUT`]).
///
/// If `f` completes within the deadline, returns its result or propagates any panic.
/// If `f` exceeds the deadline, panics immediately with a timeout failure.
pub fn with_timeout<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    with_timeout_duration(TEST_TIMEOUT, f)
}

/// Runs `f` on a dedicated thread with an explicit `timeout` duration.
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
