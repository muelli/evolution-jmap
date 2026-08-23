// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `oauth2_setup::discover_and_register` against `jmap-mock` — the same
//! fixtures `jmap-client`'s own `oauth_discovery.rs` drives, one layer up:
//! this crate's job is turning what those calls answer into a
//! `jmap_config::oauth2::Config`, and refusing to when a deployment cannot
//! back one.

use jmap_client::transport::UreqTransport;
use jmap_config::oauth2_setup::{Error, discover_and_register};
use jmap_mock::MockServer;
use serde_json::json;

const REDIRECT_URI: &str = "https://client.example.org/callback";

fn metadata(origin: &str) -> serde_json::Value {
    json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/authorize"),
        "token_endpoint": format!("{origin}/oauth/token"),
        "registration_endpoint": format!("{origin}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
    })
}

/// `MockServer::origin()` is `http://127.0.0.1:<port>`; discovery needs a
/// bare host and port, since [`discover_and_register`] builds the issuer
/// itself the same way every other JMAP connection in this project does.
fn host_and_port(server: &MockServer) -> (&str, u16) {
    let (host, port) = server
        .origin()
        .trim_start_matches("http://")
        .split_once(':')
        .expect("the mock's origin always names a port");
    (host, port.parse().expect("a numeric port"))
}

#[test]
fn a_deployment_offering_registration_yields_a_full_config() {
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .oauth_client_registration(|_request| (201, json!({"client_id": "abc123"})))
        .start();
    let (host, port) = host_and_port(&server);

    let config = discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect("the deployment supports OAuth 2.0 end to end");

    assert_eq!(config.client_id.as_deref(), Some("abc123"));
    assert_eq!(config.client_secret, None);
    assert_eq!(
        config.authorization_endpoint.as_deref(),
        Some(format!("{}/oauth/authorize", server.origin()).as_str())
    );
    assert_eq!(
        config.token_endpoint.as_deref(),
        Some(format!("{}/oauth/token", server.origin()).as_str())
    );
    assert_eq!(config.redirect_uri.as_deref(), Some(REDIRECT_URI));
}

#[test]
fn registration_is_asked_for_this_clients_name_and_the_given_redirect_uri() {
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .oauth_client_registration(|request| {
            assert_eq!(request["client_name"], "Evolution");
            assert_eq!(request["redirect_uris"], json!([REDIRECT_URI]));
            (201, json!({"client_id": "abc123"}))
        })
        .start();
    let (host, port) = host_and_port(&server);

    discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect("the deployment registers this client");
}

#[test]
fn a_server_issuing_a_secret_anyway_has_it_carried_into_the_config() {
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .oauth_client_registration(|_request| {
            (
                201,
                json!({"client_id": "abc123", "client_secret": "s3cret"}),
            )
        })
        .start();
    let (host, port) = host_and_port(&server);

    let config = discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect("a valid response");

    assert_eq!(config.client_secret.as_deref(), Some("s3cret"));
}

#[test]
fn registration_asks_for_the_scopes_this_client_uses_and_no_others() {
    // Two real-deployment lessons (Fastmail, `docs/OAUTH-FASTMAIL.md`):
    // a registration naming no `scope` is issued an *empty* default, which
    // RFC 6749 §3.3 lets a scope-less authorization request fall back to — a
    // token with no JMAP access at all. And a registration naming *every*
    // advertised scope inherits a default the user cannot be asked to
    // consent to: Fastmail's authorization endpoint answered
    // `error=invalid_scope` for a registered set including its MCP and
    // OpenID Connect scopes (observed live 2026-08-23). So: exactly the
    // scopes this client uses — the JMAP data scopes plus `offline_access`
    // for a refresh token — from those the deployment advertises, and the
    // same string is carried into `Config::scope` for the authorization
    // request to name explicitly.
    let server = MockServer::builder()
        .oauth_authorization_server(|origin| {
            let mut document = metadata(origin);
            document["scopes_supported"] = json!([
                "urn:ietf:params:oauth:scope:mail",
                "urn:ietf:params:oauth:scope:contacts",
                "urn:ietf:params:oauth:scope:calendars",
                "https://provider.example/dev/mcp",
                "openid",
                "profile",
                "email",
                "offline_access",
            ]);
            document
        })
        .oauth_client_registration(|request| {
            assert_eq!(
                request["scope"],
                "urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts \
                 urn:ietf:params:oauth:scope:calendars offline_access"
            );
            (201, json!({"client_id": "abc123"}))
        })
        .start();
    let (host, port) = host_and_port(&server);

    let config = discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect("the deployment registers this client");
    assert_eq!(
        config.scope.as_deref(),
        Some(
            "urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts \
             urn:ietf:params:oauth:scope:calendars offline_access"
        ),
        "the registered scope must be carried into the config for the \
         authorization request to name explicitly"
    );
}

#[test]
fn registration_names_no_scope_when_the_deployment_advertises_none() {
    // `metadata()` publishes no `scopes_supported` — the pure RFC 8620
    // deployment this crate has always supported — and that must keep
    // sending no `scope` at all, not an empty string.
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .oauth_client_registration(|request| {
            assert!(
                request.get("scope").is_none(),
                "expected no scope field, got {request:?}"
            );
            (201, json!({"client_id": "abc123"}))
        })
        .start();
    let (host, port) = host_and_port(&server);

    discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect("the deployment registers this client");
}

#[test]
fn a_deployment_with_no_oauth2_metadata_is_a_client_error() {
    let server = MockServer::builder().start();
    let (host, port) = host_and_port(&server);

    let error = discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect_err("no metadata is published");

    assert!(
        matches!(
            error,
            Error::Client(jmap_client::Error::Http { status: 404, .. })
        ),
        "got {error:?}"
    );
}

#[test]
fn a_deployment_without_the_authorization_code_grant_is_refused() {
    let server = MockServer::builder()
        .oauth_authorization_server(|origin| {
            let mut document = metadata(origin);
            document["grant_types_supported"] = json!(["client_credentials"]);
            document
        })
        .start();
    let (host, port) = host_and_port(&server);

    let error = discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect_err("the deployment does no authorization-code flow");

    assert!(matches!(error, Error::UnsupportedGrant), "got {error:?}");
}

#[test]
fn a_deployment_with_no_registration_endpoint_is_refused() {
    let server = MockServer::builder()
        .oauth_authorization_server(|origin| {
            let mut document = metadata(origin);
            document
                .as_object_mut()
                .expect("the document is an object")
                .remove("registration_endpoint");
            document
        })
        .start();
    let (host, port) = host_and_port(&server);

    let error = discover_and_register(
        &UreqTransport::default(),
        host,
        port,
        false,
        REDIRECT_URI,
        None,
    )
    .expect_err("the deployment offers no dynamic registration");

    assert!(matches!(error, Error::NoRegistration), "got {error:?}");
}

#[test]
fn an_insecure_non_loopback_host_is_refused_before_any_network_call() {
    // The same TLS rule every other JMAP connection in this project is held
    // to (`jmap_backend_core::source::origin`); a JMAP mock always runs
    // plaintext, so the host is deliberately not the mock's own loopback
    // address here.
    let error = discover_and_register(
        &UreqTransport::default(),
        "jmap.example.com",
        0,
        false,
        REDIRECT_URI,
        None,
    )
    .expect_err("plaintext to a non-loopback host is refused");

    assert!(matches!(error, Error::Host(_)), "got {error:?}");
}
