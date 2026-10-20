// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read the binding of the domain without a change to it.
//!
//! [`bind`] and [`bind_to`] both return the binding that they made. This is
//! sufficient for a caller that made a binding. It is not sufficient for a
//! caller that must know what the binding *is*. A test must ask a module entry
//! point "did you bind the domain?" without a bind of its own, because that
//! bind makes the answer yes in all cases. [`binding`] gives this answer.
//! `tests/textdomain.rs` in `jmap-mail` uses it to observe the provider module.
//!
//! ## Why this is a test binary of its own
//!
//! The test binds the domain to an intentionally wrong directory. This is
//! process-global state, and a sibling test would race for it. Cargo gives
//! each file in `tests/` its own process, so this file alone gives the
//! isolation. `tests/catalogue.rs` is separate for the same reason.
//!
//! [`bind`]: jmap_backend_core::i18n::bind
//! [`bind_to`]: jmap_backend_core::i18n::bind_to
//! [`binding`]: jmap_backend_core::i18n::binding

use jmap_backend_core::i18n::{LOCALE_DIR, bind, bind_to, binding};

/// The query reports the directory of the last bind, and the query itself does
/// not bind to that directory.
///
/// The decoy is necessary. In a new process, `binding() == LOCALE_DIR` passes
/// for a `binding` that ignores gettext and returns the constant. It also
/// passes for a `binding` that binds the domain as a side effect of the read.
/// The decoy is a directory that nothing would choose, so both defects show:
/// the first fails the decoy assertion, the second fails the next assertion.
#[test]
fn the_binding_is_readable_without_being_written() {
    let decoy = c"/nonexistent/jmap-decoy-locale";

    let reported = bind_to(decoy);
    assert_eq!(reported.as_c_str(), decoy, "the decoy binding took");

    assert_eq!(
        binding().as_c_str(),
        decoy,
        "the query answers with the binding that was made, not the built-in \
         default"
    );
    assert_eq!(
        binding().as_c_str(),
        decoy,
        "asking twice does not move the binding"
    );

    // The query follows a later bind and does not cache the first answer.
    // The module entry points change the binding in this direction.
    assert_eq!(bind().as_c_str(), LOCALE_DIR);
    assert_eq!(binding().as_c_str(), LOCALE_DIR);
}
