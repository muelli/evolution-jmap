// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A lock for tests that touch GTK directly.
//!
//! GTK is not thread-safe, and the default test harness gives every test
//! its own thread. Two tests that build or destroy a GTK object at the same
//! time can corrupt process-wide GTK state and crash the whole binary. CTest
//! covers this for the installed suite by forcing `--test-threads=1`
//! (`cmake/Rust.cmake`), but a plain `cargo test -p jmap-ui` still uses the
//! default thread pool. Every test that calls `gtk_init_check` or builds a
//! GTK object takes [`lock`] first, so only one such test runs at a time;
//! tests that never touch GTK are unaffected.

use std::sync::{Mutex, MutexGuard};

static GTK_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Blocks until no other GTK test holds the lock, then holds it until the
/// returned guard drops. A poisoned lock (an earlier GTK test panicked
/// while holding it) does not stop later tests from taking their turn.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    GTK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
