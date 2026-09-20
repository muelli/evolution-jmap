// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `service::authenticate` against a real JMAP server.
//!
//! `service::authenticate` is the safe-Rust body of `authenticate_sync` — per
//! that module's own doc comment, "the two functions a test can reach without
//! a `CamelSession`... are exactly `authenticate` and `report_authentication`".
//! `tests/service.rs` already proves it thoroughly against `jmap-mockd`, and
//! `live_server_connect.rs` already proves `open_mail` (which `authenticate`
//! calls first) against real Stalwart, but nothing has ever driven
//! `authenticate` itself — the actual entry point Camel's `authenticate_sync`
//! calls, which installs the connection through the [`Connected`] trait rather
//! than a hand-built `MailSync` — against a real server. This is that
//! confirmation.
//!
//! [`Connected`]: jmap_mail::service::Connected
//!
//! ## Running it
//!
//! Same environment as the other live-server tests — see
//! `docs/manual-test-live-server.md`:
//!
//! ```console
//! $ cargo test -p jmap-mail --features testing -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
//! unset.

use std::env;

use eds_sys::CAMEL_AUTHENTICATION_REJECTED;
use jmap_backend_core::source::ConnectTarget;
use jmap_mail::connect::password_credentials;
use jmap_mail::server::ServerConfig;
use jmap_mail::service::authenticate;
use jmap_mail::store::JmapStore;

const CACHED: eds_sys::CamelStoreGetFolderInfoFlags = 0;

/// The write-test account's origin and username, or `None` if the standing
/// live-server account is not configured here. Mirrors
/// `live_server_connect.rs::write_account_config`.
fn write_account_config() -> Option<(ServerConfig, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let config = ServerConfig {
        target: ConnectTarget::Origin(origin.clone()),
        user: Some(user),
    };
    Some((config, origin))
}

/// `authenticate` opens the real account and installs it on the store, the
/// same as `open_mail` proves in `live_server_connect.rs` — but this drives
/// it through the actual `Connected`-trait wiring `authenticate_sync` uses,
/// and confirms the folder listing that installed connection now serves.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn authenticate_installs_a_working_connection_against_the_real_server() {
    let Some((config, _origin)) = write_account_config() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the live-server test");
        return;
    };
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");

    let store = JmapStore::detached();
    assert!(!store.is_connected(), "a fresh store started connected");

    authenticate(
        &*store,
        &config,
        password_credentials(config.user.as_deref(), Some(&password)),
    )
    .expect("authenticate failed against the real server");

    assert!(
        store.is_connected(),
        "authenticate did not install a connection"
    );
    store
        .folders(CACHED)
        .expect("the connection authenticate installed could not list folders");
}

/// The other half of `tests/service.rs`'s
/// `a_rejected_password_is_reported_without_a_gerror_so_camel_asks_again`,
/// against a real server's own wrong-password 401 rather than
/// `jmap-mockd`'s.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn authenticate_reports_a_real_wrong_password_as_rejected_and_holds_no_connection() {
    let Some((config, _origin)) = write_account_config() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the live-server test");
        return;
    };

    let store = JmapStore::detached();
    let outcome = authenticate(
        &*store,
        &config,
        password_credentials(
            config.user.as_deref(),
            Some("definitely-wrong-password-agent-probe"),
        ),
    );

    let error = outcome.expect_err("a wrong password against the real server was accepted");
    assert_eq!(
        error.authentication_result(),
        CAMEL_AUTHENTICATION_REJECTED,
        "a real server's wrong-password rejection must map to REJECTED, not ERROR: {error}"
    );
    assert!(
        !store.is_connected(),
        "a rejected password left a connection installed"
    );
}
