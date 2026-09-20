// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `oauth2_setup::discover_and_register` (RFC 8414 discovery, RFC 7591
//! dynamic client registration) against a real JMAP server, not
//! `jmap-mockd`.
//!
//! Every existing test of this function (`tests/oauth2_setup.rs`) drives it
//! against `jmap-mock`'s own fixtures; it has never run against a real
//! deployment's actual metadata document or registration endpoint. Needs no
//! throwaway account: both RFC 8414 discovery and RFC 7591 registration are
//! unauthenticated by design (RFC 8414 §3 — a client that had to
//! authenticate for the document could never get started), so this only
//! needs `JMAP_LIVE_SERVER_URL`, not the write-test credentials.
//!
//! ## The issuer mismatch, and why this needs its own transport
//!
//! A real deployment's issuer is normally its own configured public
//! hostname, which is often not the address a test runner actually reaches
//! it on — `docs/manual-test-live-server.md`'s `JMAP_LIVE_SERVER_REBASE_URLS`
//! exists for exactly this on the JMAP session side. RFC 8414 §3.3 requires
//! the fetched document's `issuer` to equal, byte for byte, the one
//! discovery was asked for, so there is no equivalent "rebase after the
//! fact" for OAuth discovery: the issuer string handed to
//! `discover_and_register` has to be the one the deployment actually
//! states, or its own validation rejects the document outright.
//!
//! So this test first asks `JMAP_LIVE_SERVER_URL` directly what issuer it
//! states, then calls `discover_and_register` with that issuer's own host
//! while routing every request through a wrapping [`Transport`] that
//! rewrites just the origin (that issuer's host back to
//! `JMAP_LIVE_SERVER_URL`) before forwarding to a real `UreqTransport` — the
//! issuer string `discover_and_register` builds and checks against is
//! untouched, only where the bytes are actually sent changes. Same shape as
//! `jmap-mail`'s `live_server_disconnect_window.rs`'s
//! `DelayedDeliveryTransport`: a thin wrapper around a real transport for
//! one specific rewrite.
//!
//! ## Running it
//!
//! ```console
//! $ export JMAP_LIVE_SERVER_URL=http://<stalwart-ip>:8080
//! $ cargo test -p jmap-config -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_URL` is unset.

use std::env;

use jmap_client::transport::{
    HttpMethod, HttpRequest, HttpResponse, Transport, TransportError, UreqTransport,
};
use jmap_config::oauth2_setup::discover_and_register;

const REDIRECT_URI: &str = "https://client.example.org/callback";

/// Forwards every request to a real transport, rewriting only the origin of
/// requests that name `from_origin` to `to_origin` — the reachable address
/// this runner actually has to send them to. Every other part of the
/// request (path, headers, body) is untouched, and a request that does not
/// name `from_origin` at all (the reachable origin already matches the
/// issuer, or a redirect landed somewhere else) passes through unchanged.
struct RewriteOriginTransport {
    inner: UreqTransport,
    from_origin: String,
    to_origin: String,
}

impl Transport for RewriteOriginTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let rewritten;
        let url = match request.url.strip_prefix(self.from_origin.as_str()) {
            Some(rest) => {
                rewritten = format!("{}{rest}", self.to_origin);
                rewritten.as_str()
            }
            None => request.url,
        };
        self.inner.execute(HttpRequest { url, ..request })
    }
}

/// The issuer identifier a real deployment's own OAuth 2.0 metadata states,
/// and the bare host/port/secure triple that reproduces it through
/// `source::origin` the same way `discover_and_register` builds it — or
/// `None` if `JMAP_LIVE_SERVER_URL` is not set here.
///
/// A raw, unvalidated fetch rather than `jmap_client::oauth::discover`:
/// that function itself enforces RFC 8414 §3.3 (the document's issuer must
/// equal the one it was asked for), which is exactly the check the real
/// issuer/reachable-origin mismatch would fail here. This only needs to
/// learn what issuer the deployment states, not validate it yet.
fn real_issuer() -> Option<(String, String, u16, bool)> {
    let reachable_origin = env::var("JMAP_LIVE_SERVER_URL").ok()?;

    let url = format!("{reachable_origin}/.well-known/oauth-authorization-server");
    let response = UreqTransport::default()
        .execute(HttpRequest {
            method: HttpMethod::Get,
            url: &url,
            headers: &[("Accept".to_owned(), "application/json".to_owned())],
            body: None,
            cancel: None,
            max_response_bytes: 64 * 1024,
        })
        .expect("the real deployment must answer the RFC 8414 well-known request");
    assert_eq!(
        response.status, 200,
        "the real deployment's OAuth 2.0 metadata request did not return 200"
    );
    let document: serde_json::Value =
        serde_json::from_slice(&response.body).expect("the metadata document must be valid JSON");
    let issuer = document["issuer"]
        .as_str()
        .expect("the metadata document must state an issuer")
        .to_owned();

    let (scheme, rest) = issuer
        .split_once("://")
        .expect("discover() already validated the issuer is an absolute http(s) URL");
    let secure = scheme.eq_ignore_ascii_case("https");
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            (host.to_owned(), port.parse().expect("digits only"))
        }
        _ => (rest.to_owned(), if secure { 443 } else { 80 }),
    };

    Some((issuer, host, port, secure))
}

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn discover_and_register_resolves_a_full_config_against_the_real_server() {
    let Some((issuer, host, port, secure)) = real_issuer() else {
        eprintln!("JMAP_LIVE_SERVER_URL not set; skipping the live-server test");
        return;
    };
    let reachable_origin = env::var("JMAP_LIVE_SERVER_URL").unwrap();

    let transport = RewriteOriginTransport {
        inner: UreqTransport::default(),
        from_origin: issuer.clone(),
        to_origin: reachable_origin,
    };

    let config = discover_and_register(&transport, &host, port, secure, REDIRECT_URI, None)
        .expect("discover_and_register must succeed against a real deployment's own metadata");

    let client_id = config
        .client_id
        .as_deref()
        .expect("a deployment that offers registration must yield a client_id");
    assert!(
        !client_id.is_empty(),
        "the real server issued an empty client_id"
    );
    assert!(
        config
            .authorization_endpoint
            .as_deref()
            .is_some_and(|endpoint| endpoint.starts_with(&issuer)),
        "authorization_endpoint {:?} should be rooted at the real issuer {issuer}",
        config.authorization_endpoint
    );
    assert!(
        config
            .token_endpoint
            .as_deref()
            .is_some_and(|endpoint| endpoint.starts_with(&issuer)),
        "token_endpoint {:?} should be rooted at the real issuer {issuer}",
        config.token_endpoint
    );
    assert_eq!(config.redirect_uri.as_deref(), Some(REDIRECT_URI));

    // The deployment's own `scopes_supported` (checked live, not assumed)
    // includes the JMAP data scopes and `offline_access`, so all four of
    // `discover_and_register`'s requested scopes should have been picked.
    let scope = config
        .scope
        .as_deref()
        .expect("a deployment advertising scopes_supported should yield a picked scope");
    assert!(
        scope.contains("urn:ietf:params:oauth:scope:mail"),
        "{scope}"
    );
    assert!(scope.contains("offline_access"), "{scope}");

    // RFC 8707 resource indicator: `probe_resource` follows the same
    // rewritten transport to the real server's own `.well-known/jmap`
    // redirect, landing on its session document.
    let resource = config
        .resource
        .as_deref()
        .expect("probing .well-known/jmap against a real server should find a resource");
    assert!(
        resource.ends_with("/jmap/session"),
        "resource indicator {resource:?} should name the real session URL"
    );
}
