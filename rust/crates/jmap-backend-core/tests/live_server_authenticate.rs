// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ConnectError::auth_result`'s REJECTED classification against a real
//! server's own wrong-password response, not a hand-fabricated one.
//!
//! Every other test that exercises `is_wrong_password`/`auth_result`
//! (`tests/error.rs`, `tests/oauth2.rs`, `jmap-backend-collection`'s
//! `authenticate.rs`) builds its own `jmap_client::Error::Http { status: 401,
//! .. }` by hand. That is the value the module's own docs call the most
//! dangerous one to get wrong: REJECTED discards a stored password and asks
//! again, so a server that answers a bad login with something other than a
//! bare 401 would mean this crate never re-prompts, or re-prompts for a
//! password that was never the problem. Nothing before this file has checked
//! what a real deployment actually sends.
//!
//! ## Running it
//!
//! Same environment as the `*-sync` crates' live-server suites (see
//! `docs/manual-test-live-server.md`):
//!
//! ```console
//! $ export JMAP_LIVE_SERVER_URL=https://jmap.example.com
//! $ export JMAP_LIVE_SERVER_WRITE_USER=me@example.com
//! $ export JMAP_LIVE_SERVER_WRITE_PASSWORD=...
//! $ cargo test -p jmap-backend-core -- --ignored
//! ```
//!
//! Read-only: this only ever attempts to authenticate, with the real
//! username and a password that is never the real one, so it needs no
//! throwaway account of its own and never reads
//! `JMAP_LIVE_SERVER_WRITE_PASSWORD`. Skipped, not failed, when
//! `JMAP_LIVE_SERVER_WRITE_USER` is unset, matching every other live-server
//! test in this repository.

use std::env;

use eds_sys::E_SOURCE_AUTHENTICATION_REJECTED;
use jmap_backend_core::connect::{ConnectError, is_wrong_password};
use jmap_client::{Client, Credentials};

/// The real server's origin and a real, currently valid username on it, or
/// `None` if the standing write-test account is not configured here. The
/// password is deliberately not read: this test never wants the real one.
fn write_account_origin_and_user() -> Option<(String, String)> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    Some((origin, user))
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_real_servers_wrong_password_rejection_is_classified_as_rejected() {
    let Some((origin, user)) = write_account_origin_and_user() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the live-server test");
        return;
    };
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let outcome = Client::builder().rebase_urls_to_origin(rebase).connect(
        &origin,
        Credentials::basic(user, "definitely-wrong-password-agent-probe".to_owned()),
    );

    let error = outcome.expect_err("a wrong password must not be accepted by a real server");
    assert!(
        is_wrong_password(&error),
        "the real server's rejection did not classify as a wrong password: {error:?}"
    );
    assert_eq!(
        ConnectError::from(error).auth_result(),
        E_SOURCE_AUTHENTICATION_REJECTED
    );
}
