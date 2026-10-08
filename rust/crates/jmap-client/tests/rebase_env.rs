// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `rebase_urls_from_env`'s `JMAP_LIVE_SERVER_REBASE_URLS` parsing.
//!
//! One test, one process (a fresh binary per `tests/*.rs` file): the function
//! reads real process environment, and this file is the only place that
//! touches this variable, so there is no race with another test reading or
//! writing it concurrently.

use jmap_client::rebase_urls_from_env;

const VAR: &str = "JMAP_LIVE_SERVER_REBASE_URLS";

#[test]
fn rebase_urls_from_env_and_client_builder() {
    use jmap_client::ClientBuilder;

    // SAFETY: this test is the only test in this binary, so no concurrent threads mutate VAR.
    unsafe {
        std::env::remove_var(VAR);
    }
    assert!(!rebase_urls_from_env(), "unset means off");

    let builder_unset = ClientBuilder::default();
    assert!(format!("{builder_unset:?}").contains("rebase_urls_to_origin: false"));

    for truthy in ["1", "true", "TRUE", "True"] {
        unsafe {
            std::env::set_var(VAR, truthy);
        }
        assert!(rebase_urls_from_env(), "{truthy:?} should enable rebasing");
    }

    let builder_set = ClientBuilder::default();
    assert!(format!("{builder_set:?}").contains("rebase_urls_to_origin: true"));

    for falsy in ["0", "false", "yes", ""] {
        unsafe {
            std::env::set_var(VAR, falsy);
        }
        assert!(
            !rebase_urls_from_env(),
            "{falsy:?} should not enable rebasing"
        );
    }

    unsafe {
        std::env::remove_var(VAR);
    }
}
