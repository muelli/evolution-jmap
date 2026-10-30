// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `oauth2_setup::discover_and_register` (RFC 8414 discovery, RFC 7591
//! dynamic client registration) against a real JMAP server, not
//! `jmap-mockd`.
//!
//! The other tests of this function (`tests/oauth2_setup.rs`) use the fixtures
//! of `jmap-mock`. They never use the metadata document or registration
//! endpoint of a real deployment. This test needs no temporary account: RFC
//! 8414 discovery and RFC 7591 registration need no authentication by design
//! (RFC 8414 §3: a client that must authenticate for the document can never
//! start). Thus this test needs only `JMAP_LIVE_SERVER_URL`, not the
//! write-test credentials.
//!
//! ## The issuer mismatch, and why this needs its own transport
//!
//! The issuer of a real deployment is usually its configured public host name.
//! Frequently, this is not the address that a test runner uses to reach it.
//! On the JMAP session side, `JMAP_LIVE_SERVER_REBASE_URLS` in
//! `docs/manual-test-live-server.md` is for this problem. RFC 8414 §3.3 requires
//! that the `issuer` of the fetched document is byte for byte equal to the
//! issuer that discovery asked for. Thus OAuth discovery has no equivalent
//! "rebase after the fetch". The issuer string given to `discover_and_register`
//! must be the issuer that the deployment states, or its validation rejects
//! the document.
//!
//! Thus this test first asks `JMAP_LIVE_SERVER_URL` directly for its issuer.
//! Then it calls `discover_and_register` with the host of that issuer, and
//! sends each request through a wrapper [`Transport`]. The wrapper changes
//! only the origin (from the issuer host to `JMAP_LIVE_SERVER_URL`) and then
//! forwards to a real `UreqTransport`. The issuer string that
//! `discover_and_register` builds and checks does not change. Only the
//! destination of the bytes changes. This has the same shape as
//! `DelayedDeliveryTransport` in `live_server_disconnect_window.rs` of
//! `jmap-mail`: a thin wrapper around a real transport for one rewrite.
//!
//! ## Running it
//!
//! ```console
//! $ export JMAP_LIVE_SERVER_URL=http://<stalwart-ip>:8080
//! $ cargo test -p jmap-config -- --ignored
//! ```
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_URL` is unset, or when the
//! deployment publishes no RFC 8414 metadata document at all.

use std::env;

use jmap_client::transport::{
    HttpMethod, HttpRequest, HttpResponse, Transport, TransportError, UreqTransport,
};
use jmap_config::config_lookup::REDIRECT_URI;
use jmap_config::oauth2_setup::discover_and_register;

/// Forwards each request to a real transport. In a request that names
/// `from_origin`, it changes only the origin to `to_origin` (the address that
/// this runner can reach). The other parts of the request (path, headers,
/// body) do not change. A request that does not name `from_origin` (because
/// the reachable origin is the issuer, or a redirect went to a different
/// location) passes through unchanged.
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

/// The issuer identifier that the OAuth 2.0 metadata of a real deployment
/// states, and the bare host/port/secure triple that gives it again through
/// `source::origin`, as `discover_and_register` builds it. `None` if
/// `JMAP_LIVE_SERVER_URL` is not set.
///
/// A raw fetch with no validation, not `jmap_client::oauth::discover`. That
/// function enforces RFC 8414 §3.3 (the issuer of the document must be equal
/// to the issuer that it asked for), and the mismatch of the real issuer and
/// the reachable origin fails that check. This function must only learn the
/// issuer that the deployment states, not validate it.
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
    // RFC 8414 Section 3 does not require every deployment to publish this
    // document; a 404 here means the deployment just does not offer OAuth
    // 2.0 discovery, not a broken response. Treat it the same as the env
    // var being unset: skip, do not fail.
    if response.status == 404 {
        return None;
    }
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
        eprintln!(
            "JMAP_LIVE_SERVER_URL not set, or the deployment offers no RFC 8414 metadata document; \
             skipping the live-server test"
        );
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

    // The `scopes_supported` of the deployment (checked live, not assumed)
    // includes the JMAP data scopes and `offline_access`. Thus all four
    // scopes that `discover_and_register` requests must be in the result.
    let scope = config
        .scope
        .as_deref()
        .expect("a deployment advertising scopes_supported should yield a picked scope");
    assert!(
        scope.contains("urn:ietf:params:oauth:scope:mail"),
        "{scope}"
    );
    assert!(scope.contains("offline_access"), "{scope}");

    // RFC 8707 resource indicator: through the same rewrite transport,
    // `probe_resource` follows the `.well-known/jmap` redirect of the real
    // server to its session document.
    let resource = config
        .resource
        .as_deref()
        .expect("probing .well-known/jmap against a real server should find a resource");
    assert!(
        resource.ends_with("/jmap/session"),
        "resource indicator {resource:?} should name the real session URL"
    );
}
