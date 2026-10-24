// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shared head of the module entry points: a panic guard, then logging and
//! the gettext binding, then the registrations of the module.
//!
//! This is a test binary of its own because it changes the binding of the
//! domain, which is process-global. One test function keeps the steps in
//! sequence: `bind` binds at most one time for each process.

use std::cell::Cell;

use jmap_backend_core::i18n::{LOCALE_DIR, bind_to, binding};
use jmap_backend_core::module;

#[test]
fn load_prepares_the_process_then_registers_once_and_contains_a_panic() {
    let decoy = c"/nonexistent/jmap-decoy-locale";
    assert_eq!(bind_to(decoy).as_c_str(), decoy, "the decoy binding took");

    // The guard catches a panic in the closure, so the closure only records.
    let seen = Cell::new(None);
    let calls = Cell::new(0);
    module::load("module_load test", || {
        calls.set(calls.get() + 1);
        seen.set(Some((binding(), tracing::dispatcher::has_been_set())));
    });
    assert_eq!(calls.get(), 1, "the registrations run one time");
    let (bound, logging) = seen.take().expect("the registrations ran");
    assert_eq!(
        bound.as_c_str(),
        LOCALE_DIR,
        "the domain is bound before the registrations"
    );
    assert!(logging, "logging is set up before the registrations");

    // A panic in the registrations stays in the guard and does not unwind to C.
    module::load("module_load test (panic)", || {
        panic!("a registration failed")
    });
}
