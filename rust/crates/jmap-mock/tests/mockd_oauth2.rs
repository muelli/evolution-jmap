// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify `--oauth2` discovery and registration on standalone `jmap-mockd`.
//! Use raw `TcpStream` I/O for CLI-only behavior without extra HTTP dependencies.

mod common;

use common::Mockd;

#[test]
fn without_the_flag_the_metadata_endpoint_is_not_published() {
    let mockd = Mockd::spawn(&[]);

    let (status, _, _) = mockd.request("GET", "/.well-known/oauth-authorization-server", b"");

    assert!(status.contains("404"), "{status}");
}

#[test]
fn the_oauth2_flag_publishes_a_matching_metadata_document() {
    let mockd = Mockd::spawn(&["--oauth2"]);

    let (status, _, body) = mockd.request("GET", "/.well-known/oauth-authorization-server", b"");

    assert!(status.contains("200"), "{status}");
    let document: serde_json::Value = serde_json::from_str(&body).expect("a JSON document");
    assert_eq!(document["issuer"], mockd.origin);
    assert_eq!(
        document["registration_endpoint"],
        format!("{}/oauth/register", mockd.origin)
    );
    assert_eq!(document["grant_types_supported"][0], "authorization_code");
}

#[test]
fn the_oauth2_flag_also_answers_client_registration() {
    let mockd = Mockd::spawn(&["--oauth2"]);

    let (status, _, body) = mockd.request("POST", "/oauth/register", br#"{"client_name":"test"}"#);

    assert!(status.contains("201"), "{status}");
    let response: serde_json::Value = serde_json::from_str(&body).expect("a JSON document");
    assert!(
        response["client_id"].is_string(),
        "expected a client_id, got {response}"
    );
}
