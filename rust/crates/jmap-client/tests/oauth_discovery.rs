// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding a JMAP deployment's OAuth 2.0 endpoints (RFC 8414) and, where it
//! offers one, registering with it (RFC 7591), against the mock.
//!
//! The unit tests next to the code cover URL construction and the shapes of a
//! hostile document; these cover the part only a server can show — that the
//! document is fetched from the right place, that fetching it needs no
//! credentials, and that a deployment offering no OAuth 2.0 (or no
//! registration) says so.

use jmap_client::oauth::{self, AuthorizationServer, ClientRegistrationRequest};
use jmap_client::transport::UreqTransport;
use jmap_mock::MockServer;
use serde_json::json;

/// What a JMAP deployment that supports OAuth 2.0 publishes.
fn metadata(origin: &str) -> serde_json::Value {
    json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/authorize"),
        "token_endpoint": format!("{origin}/oauth/token"),
        "registration_endpoint": format!("{origin}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
    })
}

fn discover(origin: &str) -> Result<AuthorizationServer, jmap_client::Error> {
    oauth::discover(&UreqTransport::default(), origin, None)
}

#[test]
fn discovery_reads_the_deployments_own_endpoints() {
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .start();

    let discovered = discover(server.origin()).expect("the deployment publishes metadata");

    assert_eq!(discovered.issuer, server.origin());
    assert_eq!(
        discovered.authorization_endpoint.as_deref(),
        Some(format!("{}/oauth/authorize", server.origin()).as_str())
    );
    assert_eq!(
        discovered.token_endpoint.as_deref(),
        Some(format!("{}/oauth/token", server.origin()).as_str())
    );
    assert_eq!(
        discovered.registration_endpoint.as_deref(),
        Some(format!("{}/oauth/register", server.origin()).as_str())
    );
    assert_eq!(
        discovered.grant_types_supported,
        ["authorization_code", "refresh_token"]
    );
    assert_eq!(discovered.code_challenge_methods_supported, ["S256"]);
    assert!(
        discovered
            .scopes_supported
            .iter()
            .any(|scope| scope == "urn:ietf:params:jmap:mail")
    );
}

#[test]
fn the_metadata_document_is_served_without_credentials() {
    // The whole point of discovery is that it happens *before* there is a
    // token to authenticate with, so RFC 8414 §3 has the document publicly
    // readable. A server that demands credentials for everything else must
    // still answer this, and the client must ask without any.
    let server = MockServer::builder()
        .basic_auth("alice", "secret")
        .oauth_authorization_server(metadata)
        .start();

    let discovered = discover(server.origin()).expect("metadata needs no credentials");

    assert_eq!(discovered.issuer, server.origin());
}

#[test]
fn a_document_naming_another_issuer_is_refused() {
    // RFC 8414 §3.3: the `issuer` that comes back must be the one the
    // well-known URI was built from, or the document must not be used. This is
    // the mix-up defence — without it, a deployment could hand the client
    // another authorization server's endpoints and collect the code.
    let server = MockServer::builder()
        .oauth_authorization_server(|origin| {
            let mut document = metadata(origin);
            document["issuer"] = json!("https://idp.attacker.example");
            document
        })
        .start();

    let error = discover(server.origin()).expect_err("the issuer disagrees");

    assert!(
        matches!(&error, jmap_client::Error::Protocol(message)
            if message.contains("issuer") && message.contains("idp.attacker.example")),
        "expected an issuer-mismatch protocol error, got {error:?}"
    );
}

#[test]
fn a_deployment_that_offers_no_oauth2_says_so() {
    // Not every JMAP server does OAuth 2.0 — the mock's own default is one
    // that does not — and the caller has to be able to tell "no OAuth 2.0
    // here" from "the network is down".
    let server = MockServer::builder().start();

    let error = discover(server.origin()).expect_err("no metadata is published");

    assert!(
        matches!(error, jmap_client::Error::Http { status: 404, .. }),
        "expected HTTP 404, got {error:?}"
    );
}

#[test]
fn a_server_naming_no_grant_types_gets_rfc_8414s_default() {
    // §2: omitting `grant_types_supported` means `["authorization_code",
    // "implicit"]`. A caller checking whether the deployment does the
    // authorization-code flow would otherwise read the omission as "no".
    let server = MockServer::builder()
        .oauth_authorization_server(|origin| {
            let mut document = metadata(origin);
            document
                .as_object_mut()
                .expect("the document is an object")
                .remove("grant_types_supported");
            document
        })
        .start();

    let discovered = discover(server.origin()).expect("the deployment publishes metadata");

    assert_eq!(
        discovered.grant_types_supported,
        ["authorization_code", "implicit"]
    );
}

fn register(
    endpoint: &str,
    request: &ClientRegistrationRequest<'_>,
) -> Result<oauth::ClientRegistration, jmap_client::Error> {
    oauth::register_client(&UreqTransport::default(), endpoint, request, None)
}

#[test]
fn registration_reads_back_the_client_id() {
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .oauth_client_registration(|_request| (201, json!({"client_id": "abc123"})))
        .start();

    let request = ClientRegistrationRequest {
        client_name: "Evolution",
        redirect_uris: &["https://client.example.org/callback"],
        scope: None,
    };
    let registered = register(&format!("{}/oauth/register", server.origin()), &request)
        .expect("the deployment registers this client");

    assert_eq!(registered.client_id, "abc123");
    assert_eq!(registered.client_secret, None);
}

#[test]
fn registration_sends_the_client_name_redirect_uris_and_a_public_clients_metadata() {
    // RFC 8252 §8.4: a native app registers as a public client (no secret it
    // could keep confidential) and relies on PKCE instead — this is the one
    // thing only a server-side assertion on the actual request body can
    // prove, unlike the response shapes the unit tests already cover.
    let server = MockServer::builder()
        .oauth_client_registration(|request| {
            assert_eq!(request["client_name"], "Evolution");
            assert_eq!(
                request["redirect_uris"],
                json!(["https://client.example.org/callback"])
            );
            assert_eq!(request["token_endpoint_auth_method"], "none");
            assert_eq!(
                request["grant_types"],
                json!(["authorization_code", "refresh_token"])
            );
            assert_eq!(request["response_types"], json!(["code"]));
            assert!(
                request.get("scope").is_none(),
                "a request naming no scope must omit the field, not send it empty: {request:?}"
            );
            (201, json!({"client_id": "abc123"}))
        })
        .start();

    let request = ClientRegistrationRequest {
        client_name: "Evolution",
        redirect_uris: &["https://client.example.org/callback"],
        scope: None,
    };
    register(&format!("{}/oauth/register", server.origin()), &request)
        .expect("the deployment registers this client");
}

#[test]
fn a_named_scope_is_sent_as_the_registrations_default() {
    // Confirmed against a real deployment (Fastmail, 2026-08-19): a
    // registration naming no `scope` is issued an *empty* default scope, and
    // RFC 6749 §3.3 lets an authorization request that itself omits `scope`
    // fall back to exactly that registered default — silently producing a
    // token with no JMAP access at all. A caller that discovered a non-empty
    // `scopes_supported` must be able to ask for it here instead.
    let server = MockServer::builder()
        .oauth_client_registration(|request| {
            assert_eq!(
                request["scope"],
                "urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts"
            );
            (201, json!({"client_id": "abc123"}))
        })
        .start();

    let request = ClientRegistrationRequest {
        client_name: "Evolution",
        redirect_uris: &["https://client.example.org/callback"],
        scope: Some("urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts"),
    };
    register(&format!("{}/oauth/register", server.origin()), &request)
        .expect("the deployment registers this client");
}

#[test]
fn registration_is_served_without_credentials() {
    // Registration is how a client obtains an identity in the first place —
    // sending credentials for it would be circular, the same reasoning
    // `the_metadata_document_is_served_without_credentials` establishes for
    // discovery.
    let server = MockServer::builder()
        .basic_auth("alice", "secret")
        .oauth_client_registration(|_request| (201, json!({"client_id": "abc123"})))
        .start();

    let request = ClientRegistrationRequest {
        client_name: "Evolution",
        redirect_uris: &["https://client.example.org/callback"],
        scope: None,
    };
    let registered = register(&format!("{}/oauth/register", server.origin()), &request)
        .expect("registration needs no credentials");

    assert_eq!(registered.client_id, "abc123");
}

#[test]
fn a_deployment_that_offers_no_registration_says_so() {
    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .start();

    let request = ClientRegistrationRequest {
        client_name: "Evolution",
        redirect_uris: &["https://client.example.org/callback"],
        scope: None,
    };
    let error = register(&format!("{}/oauth/register", server.origin()), &request)
        .expect_err("no registration endpoint exists");

    assert!(
        matches!(error, jmap_client::Error::Http { status: 404, .. }),
        "expected HTTP 404, got {error:?}"
    );
}
