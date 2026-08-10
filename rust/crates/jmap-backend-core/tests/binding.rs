// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading the domain's binding back without disturbing it.
//!
//! [`bind`] and [`bind_to`] both answer with the binding they made, which is
//! enough for a caller that has just made one. It is not enough for a caller
//! that wants to know what the binding *is* — a module's entry point has to be
//! able to be asked "did you bind the domain?" by something that must not bind
//! it itself, because binding it would make the answer yes either way. That is
//! [`binding`], and it is what `jmap-mail`'s `tests/textdomain.rs` observes the
//! provider module through.
//!
//! ## Why this is a test binary of its own
//!
//! It binds the domain somewhere deliberately wrong, which is process-global
//! state a sibling test would race for. Cargo gives each file in `tests/` its
//! own process, so being alone in this file is the isolation — the same reason
//! `tests/catalogue.rs` is separate.
//!
//! [`bind`]: jmap_backend_core::i18n::bind
//! [`bind_to`]: jmap_backend_core::i18n::bind_to
//! [`binding`]: jmap_backend_core::i18n::binding

use jmap_backend_core::i18n::{LOCALE_DIR, bind, bind_to, binding};

/// The query reports the directory that was last bound, and does not become
/// that directory by being asked.
///
/// The decoy is the whole point. Asserting `binding() == LOCALE_DIR` on a
/// freshly started process would pass against a `binding` that ignored gettext
/// and returned the constant, and would pass again against one that bound the
/// domain as a side effect of reading it. Starting from a directory nothing
/// would ever choose means both of those are visible: the first fails the
/// decoy assertion, the second fails the one after it.
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

    // And it follows a later binding rather than caching the first answer,
    // which is the direction the module entry points move it in.
    assert_eq!(bind().as_c_str(), LOCALE_DIR);
    assert_eq!(binding().as_c_str(), LOCALE_DIR);
}
