// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests RFC 8620 section 2.2 SRV autodiscovery (`_jmap._tcp.<domain>`).
//!
//! [`ClientBuilder::connect_domain`] queries SRV records before falling back to the bare-domain URL.
//! Uses fake resolver and transport implementations to verify discovery without live network calls.

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
        ["https://api.example.com:443/.well-known/jmap"],
        "session discovery should be asked of the SRV target, not the bare domain"
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
