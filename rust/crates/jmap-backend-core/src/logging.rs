// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where this project's `tracing` events go.
//!
//! Every module in this repository is dlopened into a process it does not
//! own, and `tracing`'s global dispatcher is a statically-linked crate's own
//! state, not something the OS shares across separately-linked `.so` files —
//! so, exactly like [`crate::i18n`]'s gettext binding, each module's own copy
//! of this crate has to set its *own* dispatcher up, and has to tolerate
//! being asked to more than once.
//!
//! ## Where events end up
//!
//! [`init`] tries [`tracing_journald::layer`] first, so a factory process's
//! events land in `journalctl` structured the same way the rest of the
//! system's do. That fails wherever there is no journald socket to connect to
//! (a container, a dev shell with no systemd) — a fact worth recording, not
//! panicking over, so [`init`] falls back to plain formatted lines on stderr,
//! which every one of Evolution's factory processes already has captured by
//! whatever launched it.
//!
//! ## What is *not* here
//!
//! Every call site's conversion from the ad-hoc `g_log`-backed
//! [`crate::trampoline::log_critical`] onto structured `tracing` events with
//! real fields (account id, JMAP method, object type, request id) — this
//! module is the plumbing that makes that possible, not the conversion
//! itself. `log_critical` already emits a `tracing::error!` event as well as
//! its `g_log` call, so every one of its ~23 existing call sites is on the
//! journald path as soon as a module calls [`init`]; giving any of them their
//! own structured fields is later, per-site work.

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// The env var that raises or lowers the level, e.g. `EVOLUTION_JMAP_LOG=trace`.
///
/// Read once, at [`init`] time — like every other EDS module setting, this is
/// not meant to be changed while a factory process is already running.
pub const LOG_ENV_VAR: &str = "EVOLUTION_JMAP_LOG";

/// The directive `init` falls back to when [`LOG_ENV_VAR`] is unset, empty, or
/// not a directive `EnvFilter` accepts.
const DEFAULT_DIRECTIVE: &str = "warn";

static INIT: OnceLock<()> = OnceLock::new();

/// Sets this process's (or, more precisely, this linked copy's) `tracing`
/// dispatcher up, once.
///
/// Call it from a module's entry point, the same place and for the same
/// reason [`crate::i18n::bind`] is called: before anything can emit an event
/// worth keeping, and tolerant of running again because EDS may use and
/// unuse a module — or a process may hold several of this repository's
/// modules — more than once.
///
/// A second or later call is a deliberate no-op, not a retry: a process that
/// already has a dispatcher (its own, or a host application's) keeps it.
pub fn init() {
    INIT.get_or_init(|| {
        install(directive(std::env::var(LOG_ENV_VAR).ok().as_deref()));
    });
}

/// The `EnvFilter` directive [`init`] builds, given [`LOG_ENV_VAR`]'s value.
///
/// Separate from [`init`] so the decision — what the env var means — can be
/// tested without touching the process-global dispatcher a test process
/// shares across every test in the same binary.
fn directive(env_value: Option<&str>) -> &str {
    match env_value {
        Some(value) if !value.trim().is_empty() => value,
        _ => DEFAULT_DIRECTIVE,
    }
}

/// Builds the filtered subscriber and installs it, preferring journald.
///
/// Errors from either the journald connection or `try_init` itself (a
/// dispatcher already set, e.g. by a host application, or by another of this
/// process's own copies of this crate racing `init`) are swallowed: this
/// function's only job is to try, not to make logging mandatory for a module
/// to keep working.
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

    /// `init` must be safe to call more than once — the situation every
    /// module's own entry point is in, since EDS may use and unuse a module,
    /// and a process may hold several of this repository's modules, each
    /// assuming it might be the first.
    #[test]
    fn init_twice_does_not_panic() {
        init();
        init();
    }
}
