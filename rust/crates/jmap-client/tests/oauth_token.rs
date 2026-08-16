// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Redeeming an authorization code (RFC 6749 §4.1.3, with RFC 7636 PKCE) and
//! a refresh token (RFC 6749 §6) against the mock's token endpoint.
//!
//! The unit tests next to the code cover response parsing and PKCE
//! verifier/challenge construction; these cover the part only a server can
//! show — exactly which form fields a client sends, and that the PKCE
//! verifier really does hash to the challenge a prior authorization step
//! would have carried.

use base64::Engine as _;
use jmap_client::oauth::{self, PkceVerifier};
use jmap_client::transport::UreqTransport;
use jmap_mock::MockServer;
use serde_json::json;
use sha2::{Digest, Sha256};

fn transport() -> UreqTransport {
    UreqTransport::default()
}

#[test]
fn exchange_code_sends_the_grant_type_code_redirect_uri_client_id_and_verifier() {
    let verifier = PkceVerifier::generate();
    let expected_verifier = verifier.secret().to_owned();
    let server = MockServer::builder()
        .oauth_token(move |fields| {
            assert_eq!(fields["grant_type"], "authorization_code");
            assert_eq!(fields["code"], "auth-code-123");
            assert_eq!(
                fields["redirect_uri"],
                "https://client.example.org/callback"
            );
            assert_eq!(fields["client_id"], "abc123");
            assert_eq!(fields["code_verifier"], expected_verifier);
            (200, json!({"access_token": "tok", "token_type": "bearer"}))
        })
        .start();

    let response = oauth::exchange_code(
        &transport(),
        &format!("{}/oauth/token", server.origin()),
        "abc123",
        "auth-code-123",
        "https://client.example.org/callback",
        &verifier,
        None,
    )
    .expect("the deployment redeems the code");

    assert_eq!(response.access_token, "tok");
}

#[test]
fn the_pkce_verifier_sent_hashes_to_the_challenge_a_prior_authorization_step_carried() {
    // Simulates the two-step flow: a challenge would have been sent to
    // `authorization_endpoint` earlier, and only the verifier — never the
    // challenge again — is sent here. A real server checks the pairing
    // itself; this proves the client's own hash and encoding agree with an
    // independent RFC 7636 §4.2 computation, not just with itself.
    let verifier = PkceVerifier::generate();
    let challenge_from_the_authorization_step = verifier.challenge();

    let server = MockServer::builder()
        .oauth_token(move |fields| {
            let received_verifier = fields["code_verifier"].as_bytes();
            let recomputed = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(received_verifier));
            assert_eq!(recomputed, challenge_from_the_authorization_step);
            (200, json!({"access_token": "tok", "token_type": "bearer"}))
        })
        .start();

    oauth::exchange_code(
        &transport(),
        &format!("{}/oauth/token", server.origin()),
        "abc123",
        "auth-code-123",
        "https://client.example.org/callback",
        &verifier,
        None,
    )
    .expect("the deployment redeems the code");
}

#[test]
fn refresh_access_token_sends_the_grant_type_refresh_token_and_client_id_and_no_verifier() {
    let server = MockServer::builder()
        .oauth_token(|fields| {
            assert_eq!(fields["grant_type"], "refresh_token");
            assert_eq!(fields["refresh_token"], "old-refresh-tok");
            assert_eq!(fields["client_id"], "abc123");
            assert!(
                !fields.contains_key("code_verifier"),
                "a refresh grant carries no PKCE verifier"
            );
            (
                200,
                json!({"access_token": "new-tok", "token_type": "bearer"}),
            )
        })
        .start();

    let response = oauth::refresh_access_token(
        &transport(),
        &format!("{}/oauth/token", server.origin()),
        "abc123",
        "old-refresh-tok",
        None,
    )
    .expect("the deployment issues a new access token");

    assert_eq!(response.access_token, "new-tok");
}

#[test]
fn a_refreshed_token_can_carry_a_rotated_refresh_token() {
    let server = MockServer::builder()
        .oauth_token(|_fields| {
            (
                200,
                json!({
                    "access_token": "new-tok",
                    "token_type": "bearer",
                    "refresh_token": "rotated-refresh-tok",
                }),
            )
        })
        .start();

    let response = oauth::refresh_access_token(
        &transport(),
        &format!("{}/oauth/token", server.origin()),
        "abc123",
        "old-refresh-tok",
        None,
    )
    .expect("the deployment issues a new access token");

    assert_eq!(
        response.refresh_token.as_deref(),
        Some("rotated-refresh-tok")
    );
}

#[test]
fn a_deployment_that_refuses_the_grant_reports_its_error_code() {
    let server = MockServer::builder()
        .oauth_token(|_fields| {
            (
                400,
                json!({"error": "invalid_grant", "error_description": "expired"}),
            )
        })
        .start();

    let error = oauth::refresh_access_token(
        &transport(),
        &format!("{}/oauth/token", server.origin()),
        "abc123",
        "dead-refresh-tok",
        None,
    )
    .expect_err("the refresh token is dead");

    assert!(
        matches!(&error, jmap_client::Error::OAuthTokenRefused { error, .. }
            if error == "invalid_grant"),
        "expected invalid_grant, got {error:?}"
    );
}

#[test]
fn a_deployment_offering_no_token_endpoint_says_so() {
    let server = MockServer::builder().start();

    let error = oauth::refresh_access_token(
        &transport(),
        &format!("{}/oauth/token", server.origin()),
        "abc123",
        "some-refresh-tok",
        None,
    )
    .expect_err("no token endpoint is configured");

    assert!(
        matches!(error, jmap_client::Error::Http { status: 404, .. }),
        "expected HTTP 404, got {error:?}"
    );
}
