// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smoke and regression tests for jmap-client response handling fuzzer.
//!
//! Asserts that feeding arbitrary or malformed response bodies through a fake
//! transport yields standard client errors without panicking.

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials};
use jmap_proto::Id;

const MINIMAL_AUTHENTICATED_SESSION: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:contacts":{},"urn:ietf:params:jmap:calendars":{}},"accounts":{"acc1":{"name":"user","isPersonal":true,"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:contacts":{},"urn:ietf:params:jmap:calendars":{}}}},"primaryAccounts":{"urn:ietf:params:jmap:mail":"acc1","urn:ietf:params:jmap:contacts":"acc1","urn:ietf:params:jmap:calendars":"acc1"},"username":"user","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/","state":"0"}"#;

struct MockResponseTransport {
    response_body: Vec<u8>,
}

impl Transport for MockResponseTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let (status, body) = match request.method {
            HttpMethod::Get => (200, MINIMAL_AUTHENTICATED_SESSION.as_bytes().to_vec()),
            HttpMethod::Post => (200, self.response_body.clone()),
        };
        Ok(HttpResponse {
            status,
            content_type: Some("application/json".to_owned()),
            body,
            final_url: request.url.to_owned(),
        })
    }
}

fn exercise_client_with_body(body: &[u8]) {
    let transport = MockResponseTransport {
        response_body: body.to_vec(),
    };
    if let Ok(client) = Client::builder()
        .transport(transport)
        .connect("https://mail.example.internal", Credentials::none())
    {
        let account_id = Id::new("acc1");
        let _ = client.mailbox_get(&account_id);
        let _ = client.email_get(&account_id, &[Id::new("e1")], None);
        let _ = client.calendars(&account_id);
        let _ = client.event_get(&account_id, &[Id::new("ev1")]);
        let _ = client.address_books(&account_id);
        let _ = client.contact_get(&account_id, &[Id::new("c1")]);
    }
}

#[test]
fn hostile_response_bodies_never_panic() {
    let hostile_payloads: &[&[u8]] = &[
        b"",
        b"{",
        b"[",
        b"404 Not Found",
        b"{\"methodResponses\": null}",
        b"{\"methodResponses\": [\"invalid\"]}",
        b"{\"methodResponses\": [[\"error\", {\"type\": \"serverFail\"}, \"c0\"]]}",
        b"{\"sessionState\": \"abc\"}",
        b"\xFF\xFE\x00\x00",
    ];

    for payload in hostile_payloads {
        exercise_client_with_body(payload);
    }
}
