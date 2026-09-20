// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `open_mail` against a real JMAP server, not `jmap-mockd`.
//!
//! `jmap-backend-core/tests/live_server_authenticate.rs` already proves
//! `is_wrong_password`/`ConnectError::auth_result` against a real server's
//! own wrong-password 401, but only by calling
//! `Client::builder().connect(...)` directly. `jmap-mail::connect::open_mail`
//! is the actual entry point Evolution reaches (via `MailSync`'s
//! constructor path), and it wraps that same classification into the
//! Camel-flavored `StoreError::authentication_result()`. Every existing test
//! of that mapping (`tests/connect.rs`) builds a hand-fabricated
//! `jmap_client::Error::Http { status: 401, .. }` against `jmap-mockd`.
//! Every other live-server test in this crate builds its `Client`/
//! `MailSync` directly (see `live_server_expunge.rs`'s `connect_for_write`),
//! bypassing `open_mail` entirely, so `open_mail` itself, happy path or
//! otherwise, has never run against a real server at all. This is that
//! confirmation.
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
use jmap_mail::connect::{StoreError, open_mail, password_credentials};
use jmap_mail::server::ServerConfig;
use jmap_mail_sync::MailSync;
use jmap_proto::session::CAPABILITY_MAIL;

/// `MailSync` is not `Debug`, so `Result::expect_err` cannot be used directly
/// on `open_mail`'s return value.
fn expect_error(result: Result<MailSync, StoreError>) -> StoreError {
    match result {
        Ok(sync) => panic!(
            "expected a failure, but opened account {}",
            sync.account_id()
        ),
        Err(error) => error,
    }
}

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

    let error = expect_error(result);
    assert_eq!(
        error.authentication_result(),
        CAMEL_AUTHENTICATION_REJECTED,
        "a real server's wrong-password rejection must map to REJECTED, not ERROR: {error}"
    );
}
