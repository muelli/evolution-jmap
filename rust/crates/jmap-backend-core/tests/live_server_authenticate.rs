// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The REJECTED classification of `ConnectError::auth_result`, tested with
//! the wrong-password response of a real server, not a hand-made one.
//!
//! Other tests build their own `jmap_client::Error::Http { status: 401, .. }`
//! by hand: `tests/oauth2.rs` and `authenticate.rs` in
//! `jmap-backend-collection` for `is_wrong_password`/`auth_result`, and
//! `tests/error.rs` for the GError mapping. The module docs call this value
//! the most dangerous one to get wrong. REJECTED discards a stored password
//! and asks again. If a server answers a bad login with more or other than a
//! bare 401, this crate never prompts again, or prompts for a password that
//! was correct. Before this file, no test checked what a real deployment
//! sends.
//!
//! ## Running it
//!
//! Use the same environment as the live-server suites of the `*-sync` crates
//! (see `docs/manual-test-live-server.md`):
//!
//! ```console
//! $ export JMAP_LIVE_SERVER_URL=https://jmap.example.com
//! $ export JMAP_LIVE_SERVER_WRITE_USER=me@example.com
//! $ export JMAP_LIVE_SERVER_WRITE_PASSWORD=...
//! $ cargo test -p jmap-backend-core -- --ignored
//! ```
//!
//! Read-only: the test only tries to authenticate, with the real username and
//! a password that is never the real one. Thus it needs no throwaway account
//! of its own and never reads `JMAP_LIVE_SERVER_WRITE_PASSWORD`. When
//! `JMAP_LIVE_SERVER_WRITE_USER` is unset, the test is skipped, not failed,
//! as each other live-server test in this repository.

use std::env;

use eds_sys::E_SOURCE_AUTHENTICATION_REJECTED;
use jmap_backend_core::connect::{ConnectError, is_wrong_password};
use jmap_client::{Client, Credentials};

/// The origin of the real server and a real, currently valid username on it,
/// or `None` if the standing write-test account is not configured here. The
/// password is intentionally not read: this test never uses the real one.
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
