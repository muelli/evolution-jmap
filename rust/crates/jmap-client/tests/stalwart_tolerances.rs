// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pinned mock-level regression tests verifying that `jmap-client` tolerates
//! real-world spec deviations emitted by live Stalwart deployments
//! (STALWART-RFC-FINDINGS.md, Batch 25).

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::Mailbox;
use jmap_proto::session;

/// Exact wire JSON returned by live Stalwart v1.0.0 (0.16.22) on
/// unauthenticated GET to the session resource (Finding 12).
const STALWART_UNAUTHENTICATED_SESSION: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:calendars":{},"urn:ietf:params:jmap:calendars:parse":{},"urn:ietf:params:jmap:contacts":{},"urn:ietf:params:jmap:contacts:parse":{},"urn:ietf:params:jmap:filenode":{},"urn:ietf:params:jmap:principals":{},"urn:ietf:params:jmap:principals:availability":{},"urn:ietf:params:jmap:submission":{},"urn:ietf:params:jmap:vacationresponse":{},"urn:ietf:params:jmap:sieve":{"implementation":"Stalwart v1.0.0"},"urn:ietf:params:jmap:blob":{},"urn:ietf:params:jmap:quota":{},"urn:ietf:params:jmap:emailpush":{},"urn:ietf:params:jmap:webpush-vapid":{"applicationServerKey":"BBfUuHEbvaeQ4FgVQ-ndazP3KL_-znnUXe2JUxO4kQgYGDKqdr6AbNQxmkWonaZMsXlibHLz9N8ocHKnus4wO24"},"urn:ietf:params:jmap:websocket":{"url":"wss://mail.example.internal/jmap/ws","supportsPush":true}},"accounts":{},"primaryAccounts":{},"username":"","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}","state":"0"}"#;

struct MockSessionTransport {
    body: Vec<u8>,
}

impl Transport for MockSessionTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: self.body.clone(),
            final_url: request.url.to_owned(),
        })
    }
}

#[test]
fn finding_12_unauthenticated_session_tolerated_and_detected() {
    let transport = MockSessionTransport {
        body: STALWART_UNAUTHENTICATED_SESSION.as_bytes().to_vec(),
    };
    let client = Client::builder()
        .transport(transport)
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect must parse Stalwart unauthenticated session response");

    // 1. Client parsed the session response cleanly:
    let session = client.session();
    assert_eq!(session.username, "");
    assert!(session.accounts.is_empty());
    assert!(session.primary_accounts.is_empty());
    assert!(session.capabilities.contains_key(session::CAPABILITY_CORE));
    assert!(session.capabilities.contains_key(session::CAPABILITY_MAIL));
    assert!(
        session
            .capabilities
            .contains_key(session::CAPABILITY_CALENDARS)
    );
    assert!(
        session
            .capabilities
            .contains_key(session::CAPABILITY_CONTACTS)
    );
    assert!(
        session
            .capabilities
            .contains_key(session::CAPABILITY_PRINCIPALS)
    );
    assert!(
        session
            .capabilities
            .contains_key(session::CAPABILITY_SUBMISSION)
    );

    // 2. Client identifies the session as anonymous / unauthenticated:
    assert!(
        client.is_anonymous(),
        "client must report is_anonymous() as true for an unauthenticated session"
    );

    // 3. Resolving a primary account fails cleanly with Protocol error rather than panicking:
    match client.primary_account(session::CAPABILITY_MAIL) {
        Err(Error::Protocol(msg)) => assert!(msg.contains("no primary account")),
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}

#[test]
fn authenticated_session_reports_not_anonymous() {
    let server = MockServer::builder().start();
    let client =
        Client::connect(server.origin(), Credentials::none()).expect("mock connection succeeds");
    assert!(
        !client.is_anonymous(),
        "mock server session with seeded accounts must not report is_anonymous()"
    );
}

/// Exact wire JSON returned by live Stalwart v1.0.0 (0.16.22) on
/// Mailbox/set create with whitespace-padded name (Finding 16).
/// Stalwart stores the trimmed name ("Padded Name"), but omits the name property
/// from the created map, returning only the identifier.
const STALWART_MAILBOX_SET_CREATED_OMITTING_NAME: &str = r#"{"methodResponses":[["Mailbox/set",{"accountId":"d333333","oldState":"sn2","newState":"soa","created":{"new":{"id":"m"}}},"c0"]],"sessionState":"aa288e37"}"#;

const STALWART_AUTHENTICATED_SESSION: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:mail":{}},"accounts":{"d333333":{"name":"admin","isPersonal":true,"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:mail":{}}}},"primaryAccounts":{"urn:ietf:params:jmap:mail":"d333333"},"username":"admin","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/","state":"0"}"#;

struct MockMailboxSetTransport;

impl Transport for MockMailboxSetTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let body = match request.method {
            HttpMethod::Get => STALWART_AUTHENTICATED_SESSION.as_bytes().to_vec(),
            HttpMethod::Post => STALWART_MAILBOX_SET_CREATED_OMITTING_NAME
                .as_bytes()
                .to_vec(),
        };
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body,
            final_url: request.url.to_owned(),
        })
    }
}

#[test]
fn finding_16_mailbox_set_create_omitted_trimmed_name_tolerated() {
    let client = Client::builder()
        .transport(MockMailboxSetTransport)
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");
    let requested = Mailbox::new("  Padded Name  ");
    let created = client
        .mailbox_create(&account_id, &requested)
        .expect("mailbox_create succeeds");

    // 1. The server allocated an id:
    assert_eq!(created.id, Some(Id::new("m")));

    // 2. The client layer tolerates the omitted name and trims the requested name:
    assert_eq!(
        created.name, "Padded Name",
        "mailbox_create must report the trimmed name when the server omits it"
    );
}
