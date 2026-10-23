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
///
/// Lock a test lock before you call this function. The time limit must not
/// include the wait for the lock.
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

/// A source that two threads share, as a worker thread and the main loop share
/// it in EDS.
struct Shared(*mut eds_sys::ESource);
// SAFETY: an `ESource` and its extensions guard their state with a lock.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// Two long values that a setter writes in turn. Each one ends in `stem`.
pub fn long_values(stem: &str) -> [String; 2] {
    ["a".repeat(4000) + stem, "b".repeat(9000) + stem]
}

/// Runs `read` on this thread while another thread calls `write` 20,000 times
/// with the two `values` in turn. Each `read` must give one of `allowed` or one
/// of `values`. A borrowed getter copies freed memory and gives a torn value.
pub fn read_while_another_thread_writes(
    source: *mut eds_sys::ESource,
    values: [String; 2],
    write: unsafe fn(*mut eds_sys::ESource, *const std::ffi::c_char),
    read: impl Fn(*mut eds_sys::ESource) -> Option<String>,
    allowed: &[Option<String>],
) {
    let shared = std::sync::Arc::new(Shared(source));
    let names: Vec<std::ffi::CString> = values
        .iter()
        .map(|value| std::ffi::CString::new(value.as_str()).expect("no NUL"))
        .collect();
    let writer = {
        let shared = std::sync::Arc::clone(&shared);
        thread::spawn(move || {
            for round in 0..20_000 {
                // SAFETY: a live source and a NUL-terminated value.
                unsafe { write(shared.0, names[round % 2].as_ptr()) };
            }
        })
    };
    while !writer.is_finished() {
        let value = read(shared.0);
        assert!(
            allowed.contains(&value) || values.iter().any(|v| value.as_deref() == Some(v)),
            "a torn value of {:?} bytes",
            value.map(|v| v.len())
        );
    }
    writer.join().expect("writer");
}
