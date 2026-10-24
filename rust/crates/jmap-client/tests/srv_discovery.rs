// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests RFC 8620 section 2.2 SRV autodiscovery (`_jmap._tcp.<domain>`).
//!
//! [`ClientBuilder::connect_domain`] queries SRV records before falling back to the bare-domain URL.
//! Uses fake resolver and transport implementations to verify discovery without live network calls.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use jmap_client::resolver::{Resolver, SrvTarget};
use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials};
use serde_json::json;

/// Returns a configured answer for each domain query.
struct FakeResolver(Option<SrvTarget>);

impl Resolver for FakeResolver {
    fn lookup_srv(&self, _domain: &str) -> Option<SrvTarget> {
        self.0.clone()
    }
}

/// Records requested URLs and returns a fixed session document.
/// Verifies the discovery endpoint URL without network access.
struct FakeTransport {
    seen: Arc<Mutex<Vec<String>>>,
    session_body: Vec<u8>,
}

impl Transport for FakeTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        self.seen.lock().unwrap().push(request.url.to_owned());
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: self.session_body.clone(),
            final_url: request.url.to_owned(),
        })
    }
}

fn session_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "capabilities": {},
        "accounts": {},
        "primaryAccounts": {},
        "username": "vera@example.com",
        "apiUrl": "https://jmap.example.com/jmap",
        "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}",
        "eventSourceUrl": "https://jmap.example.com/eventsource",
        "state": "s0",
    }))
    .unwrap()
}

#[test]
fn an_srv_record_redirects_session_discovery_to_its_target() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let transport = FakeTransport {
        seen: seen.clone(),
        session_body: session_body(),
    };
    let resolver = FakeResolver(Some(SrvTarget {
        host: "api.example.com".to_owned(),
        port: 443,
    }));

    Client::builder()
        .transport(transport)
        .resolver(resolver)
        .connect_domain("example.com", Credentials::none())
        .expect("session discovery against the SRV target should succeed");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["https://api.example.com/.well-known/jmap"],
        "session discovery drops port 443 from SRV target"
    );
}

#[test]
fn an_srv_record_with_custom_port_preserves_port_in_session_discovery() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let transport = FakeTransport {
        seen: seen.clone(),
        session_body: session_body(),
    };
    let resolver = FakeResolver(Some(SrvTarget {
        host: "api.example.com".to_owned(),
        port: 8443,
    }));

    Client::builder()
        .transport(transport)
        .resolver(resolver)
        .connect_domain("example.com", Credentials::none())
        .expect("session discovery against the custom port SRV target should succeed");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["https://api.example.com:8443/.well-known/jmap"],
        "session discovery should preserve custom non-443 port"
    );
}

#[test]
fn no_srv_record_falls_back_to_the_bare_domain() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let transport = FakeTransport {
        seen: seen.clone(),
        session_body: session_body(),
    };
    let resolver = FakeResolver(None);

    Client::builder()
        .transport(transport)
        .resolver(resolver)
        .connect_domain("example.com", Credentials::none())
        .expect("session discovery against the bare domain should succeed");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["https://example.com/.well-known/jmap"],
        "with no SRV record, today's bare-domain fallback must be unchanged"
    );
}

#[test]
fn connect_domain_without_a_custom_resolver_uses_the_bare_domain() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let transport = FakeTransport {
        seen: seen.clone(),
        session_body: session_body(),
    };

    Client::builder()
        .transport(transport)
        .connect_domain("example.com", Credentials::none())
        .expect("session discovery against the bare domain should succeed");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["https://example.com/.well-known/jmap"],
        "the default resolver does no SRV lookup, matching today's behaviour"
    );
}

/// A transport simulating an SRV session endpoint on port 443 that redirects
/// to the same host without port, preserving credentials only if same_origin returns true.
struct SrvRedirectTransport {
    initial_url: Arc<Mutex<Option<String>>>,
    redirect_received_auth: Arc<AtomicBool>,
    session_body: Vec<u8>,
}

impl Transport for SrvRedirectTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let auth_header = request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.to_string());

        let mut initial_url_guard = self.initial_url.lock().unwrap();
        if initial_url_guard.is_none() {
            *initial_url_guard = Some(request.url.to_string());
        }

        let redirect_target = "https://api.example.com/jmap/session";

        // As in UreqTransport, Authorization is stripped across cross-origin redirects.
        let same_origin = jmap_client::url::same_origin(request.url, redirect_target);
        let auth_on_redirect = if same_origin { auth_header } else { None };

        if auth_on_redirect.as_deref() == Some("Bearer secret-token-443") {
            self.redirect_received_auth.store(true, Ordering::SeqCst);
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: self.session_body.clone(),
                final_url: redirect_target.to_string(),
            })
        } else {
            Err(TransportError::Failed(
                "request to redirect target lacked authorization credentials".to_string(),
            ))
        }
    }
}

#[test]
fn srv_target_on_port_443_redirecting_to_same_host_without_port_keeps_authorization() {
    let initial_url = Arc::new(Mutex::new(None));
    let redirect_received_auth = Arc::new(AtomicBool::new(false));
    let transport = SrvRedirectTransport {
        initial_url: initial_url.clone(),
        redirect_received_auth: redirect_received_auth.clone(),
        session_body: session_body(),
    };
    let resolver = FakeResolver(Some(SrvTarget {
        host: "api.example.com".to_owned(),
        port: 443,
    }));

    let client = Client::builder()
        .transport(transport)
        .resolver(resolver)
        .connect_domain("example.com", Credentials::bearer("secret-token-443"))
        .expect("session discovery across port 443 redirect to unported host should succeed");

    assert!(
        redirect_received_auth.load(Ordering::SeqCst),
        "redirect target must receive the Authorization header"
    );
    assert_eq!(
        initial_url.lock().unwrap().as_deref(),
        Some("https://api.example.com/.well-known/jmap"),
        "connect_domain should drop port 443 from discovery URL"
    );
    assert_eq!(client.session().username, "vera@example.com");
}
