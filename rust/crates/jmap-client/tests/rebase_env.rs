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
fn parses_the_documented_truthy_and_falsy_spellings() {
    // SAFETY: this test is the only thing in this binary that touches `VAR`.
    unsafe {
        std::env::remove_var(VAR);
    }
    assert!(!rebase_urls_from_env(), "unset means off");

    for truthy in ["1", "true", "TRUE", "True"] {
        unsafe {
            std::env::set_var(VAR, truthy);
        }
        assert!(rebase_urls_from_env(), "{truthy:?} should enable rebasing");
    }

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
