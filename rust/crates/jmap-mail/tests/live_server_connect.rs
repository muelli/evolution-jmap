// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `open_mail` against a real JMAP server, not `jmap-mockd`.
//!
//! `jmap-backend-core/tests/live_server_authenticate.rs` already proves
//! `is_wrong_password`/`ConnectError::auth_result` against a real server's
//! wrong-password 401. It calls `Client::builder().connect(...)` directly.
//!
//! `jmap_mail::connect::open_mail` is the actual entry point that Evolution
//! reaches, through `MailSync`'s constructor path. It wraps the same
//! classification into the Camel-flavored `StoreError::authentication_result()`.
//! Every existing test of that mapping (`tests/connect.rs`) builds a
//! hand-fabricated `jmap_client::Error::Http { status: 401, .. }` against
//! `jmap-mockd`. Every other live-server test in this crate builds its
//! `Client`/`MailSync` directly (see `live_server_expunge.rs`'s
//! `connect_for_write`), and bypasses `open_mail` entirely. So `open_mail`
//! itself, on the happy path or otherwise, has never run against a real
//! server. This test is that confirmation.
//!
//! ## Running it
//!
//! Same environment as the other live-server tests. See
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
use jmap_mail::connect::{open_mail, password_credentials};
use jmap_mail::server::ServerConfig;
use jmap_proto::session::CAPABILITY_MAIL;

/// The write-test account's origin and username, or `None` if the standing
/// live-server account is not configured here.
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

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn open_mail_resolves_the_real_servers_primary_mail_account() {
    let Some((config, origin)) = write_account_config() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the live-server test");
        return;
    };
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");

    let sync = open_mail(
        &config,
        password_credentials(config.user.as_deref(), Some(&password)),
    )
    .expect("open_mail failed against the real server");

    // Independently confirmed by asking the server the same question
    // `open_mail` itself asks internally.
    let client = jmap_client::Client::builder()
        .rebase_urls_to_origin(env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|v| v != "0"))
        .connect(
            &origin,
            jmap_client::Credentials::basic(config.user.clone().unwrap(), password),
        )
        .expect("could not fetch the session document for the write-test account");
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account must offer mail");

    assert_eq!(
        sync.account_id(),
        &account_id,
        "open_mail must resolve to the same primary mail account the session itself names"
    );
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn open_mail_reports_a_real_wrong_password_as_rejected() {
    let Some((config, _origin)) = write_account_config() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the live-server test");
        return;
    };

    let result = open_mail(
        &config,
        password_credentials(
            config.user.as_deref(),
            Some("definitely-wrong-password-agent-probe"),
        ),
    );

    let accepts_any_password =
        env::var("JMAP_LIVE_SERVER_ACCEPTS_ANY_PASSWORD").is_ok_and(|val| val == "1");

    let error = match result {
        Ok(_) if accepts_any_password => {
            eprintln!(
                "server accepted a wrong password for user {:?}; JMAP_LIVE_SERVER_ACCEPTS_ANY_PASSWORD=1 is set, skipping rejection assertion",
                config.user
            );
            return;
        }
        Ok(_) => {
            panic!(
                "a wrong password was accepted by the server for user {:?}; if this server intentionally accepts any password, set JMAP_LIVE_SERVER_ACCEPTS_ANY_PASSWORD=1",
                config.user
            );
        }
        Err(err) => err,
    };
    assert_eq!(
        error.authentication_result(),
        CAMEL_AUTHENTICATION_REJECTED,
        "a real server's wrong-password rejection must map to REJECTED, not ERROR: {error}"
    );
}
