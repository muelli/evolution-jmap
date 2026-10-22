// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `Debug` output of a credential type must not show a secret. A log
//! line or a panic message can contain this output.

use jmap_client::Credentials;
use jmap_client::oauth::TokenResponse;

const PASSWORD: &str = "pw-7c1e9a54-secret";
const ACCESS: &str = "at-0b3d22f1-secret";
const REFRESH: &str = "rt-5e88a0c6-secret";

#[test]
fn basic_credentials_debug_shows_the_user_but_not_the_password() {
    let shown = format!("{:?}", Credentials::basic("vera@example.com", PASSWORD));
    assert!(!shown.contains(PASSWORD), "{shown}");
    assert!(shown.contains("vera@example.com"), "{shown}");
    let pretty = format!("{:#?}", Credentials::basic("vera@example.com", PASSWORD));
    assert!(!pretty.contains(PASSWORD), "{pretty}");
}

#[test]
fn bearer_credentials_debug_does_not_show_the_token() {
    let shown = format!("{:?}", Credentials::Bearer(ACCESS.to_owned()));
    assert!(!shown.contains(ACCESS), "{shown}");
    assert!(shown.contains("Bearer"), "{shown}");
}

#[test]
fn no_credentials_debug_is_unchanged() {
    assert_eq!(format!("{:?}", Credentials::none()), "None");
}

#[test]
fn token_response_debug_shows_no_token() {
    let response = TokenResponse {
        access_token: ACCESS.to_owned(),
        token_type: "Bearer".to_owned(),
        expires_in: Some(3600),
        refresh_token: Some(REFRESH.to_owned()),
        scope: Some("urn:ietf:params:jmap:core".to_owned()),
    };
    for shown in [format!("{response:?}"), format!("{response:#?}")] {
        assert!(!shown.contains(ACCESS), "{shown}");
        assert!(!shown.contains(REFRESH), "{shown}");
        assert!(shown.contains("3600"), "{shown}");
        assert!(shown.contains("urn:ietf:params:jmap:core"), "{shown}");
    }
}

#[test]
fn token_response_debug_tells_whether_a_refresh_token_is_present() {
    let mut response = TokenResponse {
        access_token: ACCESS.to_owned(),
        token_type: "Bearer".to_owned(),
        expires_in: None,
        refresh_token: Some(REFRESH.to_owned()),
        scope: None,
    };
    let with = format!("{response:?}");
    response.refresh_token = None;
    let without = format!("{response:?}");
    assert_ne!(with, without);
}
