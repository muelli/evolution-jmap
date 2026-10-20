// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the `tracing` events of this project go.
//!
//! Every module in this repository is dlopened into a process that it does
//! not own. The global dispatcher of `tracing` is state of a statically
//! linked crate, and the OS does not share it across separately linked `.so`
//! files. Thus, exactly as for the gettext binding of [`crate::i18n`], the
//! copy of this crate in each module must configure its *own* dispatcher.
//! It must also accept more than one request to do this.
//!
//! ## Where events go
//!
//! [`init`] tries [`tracing_journald::layer`] first. Thus the events of a
//! factory process go to `journalctl`, with the same structure as the events
//! of the rest of the system. This fails when there is no journald socket
//! (a container, or a dev shell with no systemd). This fact is worth a
//! record, but not a panic. Thus [`init`] then uses plain formatted lines on
//! stderr. The launcher of each Evolution factory process already captures
//! stderr.
//!
//! ## What is *not* here
//!
//! The conversion of each call site from the ad-hoc, `g_log`-based
//! [`crate::trampoline::log_critical`] to structured `tracing` events with
//! real fields (account id, JMAP method, object type, request id). This
//! module is the infrastructure for that conversion, not the conversion.
//! `log_critical` already emits a `tracing::error!` event and its `g_log`
//! call. Thus all of its existing call sites go to journald immediately
//! after a module calls [`init`]. Structured fields for each call site are
//! later work.

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// The env var that raises or lowers the level, e.g. `EVOLUTION_JMAP_LOG=trace`.
///
/// Read one time, by [`init`]. As for every other EDS module setting, do not
/// change it while a factory process runs.
pub const LOG_ENV_VAR: &str = "EVOLUTION_JMAP_LOG";

/// The directive that `init` uses when [`LOG_ENV_VAR`] is unset, empty, or
/// not a directive that `EnvFilter` accepts.
const DEFAULT_DIRECTIVE: &str = "warn";

static INIT: OnceLock<()> = OnceLock::new();

/// Configures the `tracing` dispatcher of this process (more precisely, of
/// this linked copy), one time.
///
/// Call it from the entry point of a module, before code can emit a useful
/// event. [`crate::i18n::bind`] has the same location and the same reason.
/// It accepts more than one call, because EDS can use and unuse a module more
/// than one time, and a process can hold more than one module of this
/// repository.
///
/// A second or later call intentionally does nothing. It is not a retry: a
/// process that already has a dispatcher (its own, or one from a host
/// application) keeps it.
pub fn init() {
    INIT.get_or_init(|| {
        install(directive(std::env::var(LOG_ENV_VAR).ok().as_deref()));
    });
}

/// The `EnvFilter` directive that [`init`] builds from the value of
/// [`LOG_ENV_VAR`].
///
/// This is separate from [`init`], so that a test can examine the decision
/// (the meaning of the env var) without a change to the process-global
/// dispatcher. A test process shares that dispatcher across all tests in the
/// same binary.
fn directive(env_value: Option<&str>) -> &str {
    match env_value {
        Some(value) if !value.trim().is_empty() => value,
        _ => DEFAULT_DIRECTIVE,
    }
}

/// Builds the filtered subscriber and installs it. Journald is the first
/// choice.
///
/// The function ignores errors from the journald connection and from
/// `try_init` (a dispatcher that is already set, for example by a host
/// application, or by a different copy of this crate in this process that
/// races `init`). This function must only try. A module must continue to
/// work without logging.
fn install(directive: &str) {
    let filter =
        EnvFilter::try_new(directive).unwrap_or_else(|_| EnvFilter::new(DEFAULT_DIRECTIVE));
    let registry = tracing_subscriber::registry().with(filter);
    match tracing_journald::layer() {
        Ok(layer) => {
            let _ = registry.with(layer).try_init();
        }
        Err(_) => {
            let fallback = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
            let _ = registry.with(fallback).try_init();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_falls_back_to_the_default() {
        assert_eq!(directive(None), DEFAULT_DIRECTIVE);
    }

    #[test]
    fn empty_or_blank_falls_back_to_the_default() {
        assert_eq!(directive(Some("")), DEFAULT_DIRECTIVE);
        assert_eq!(directive(Some("   ")), DEFAULT_DIRECTIVE);
    }

    #[test]
    fn a_bare_level_is_passed_through_verbatim() {
        assert_eq!(directive(Some("trace")), "trace");
    }

    #[test]
    fn a_per_target_directive_is_passed_through_verbatim() {
        assert_eq!(
            directive(Some("evolution_jmap=debug,warn")),
            "evolution_jmap=debug,warn"
        );
    }

    /// More than one call to `init` must be safe. The entry point of each
    /// module is in this situation: EDS can use and unuse a module, and a
    /// process can hold more than one module of this repository. Each module
    /// assumes that it is possibly the first.
    #[test]
    fn init_twice_does_not_panic() {
        init();
        init();
    }
}
