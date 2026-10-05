// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pinned mock-level regression tests verifying that `jmap-client` tolerates
//! real-world spec deviations emitted by live Stalwart deployments
//! (STALWART-RFC-FINDINGS.md, Batch 25).

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::ParticipantIdentity;
use jmap_proto::mail::Mailbox;
use jmap_proto::methods::GetResponse;
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

/// Exact wire JSON returned by live Stalwart v1.0.0 (0.16.22) on
/// CalendarEventNotification/set destroy with an unformatted ID (Finding 20).
/// Stalwart silently drops the unformatted ID from both destroyed and notDestroyed.
const STALWART_NOTIFICATION_SET_DESTROY_DROPPING_ID: &str = r#"{"methodResponses":[["CalendarEventNotification/set",{"accountId":"d333333"},"c0"]],"sessionState":"aa288e37"}"#;

const STALWART_AUTHENTICATED_SESSION_WITH_CALENDARS: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:calendars":{}},"accounts":{"d333333":{"name":"admin","isPersonal":true,"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:calendars":{}}}},"primaryAccounts":{"urn:ietf:params:jmap:calendars":"d333333"},"username":"admin","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/","state":"0"}"#;

struct MockNotificationSetTransport;

impl Transport for MockNotificationSetTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let body = match request.method {
            HttpMethod::Get => STALWART_AUTHENTICATED_SESSION_WITH_CALENDARS
                .as_bytes()
                .to_vec(),
            HttpMethod::Post => STALWART_NOTIFICATION_SET_DESTROY_DROPPING_ID
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
fn finding_20_calendar_event_notification_destroy_dropped_id_tolerated_as_not_found() {
    let client = Client::builder()
        .transport(MockNotificationSetTransport)
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");
    let result =
        client.calendar_event_notification_destroy(&account_id, &Id::new("unformatted_id"));

    match result {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
        }
        other => panic!("expected Error::Set with notFound, got {other:?}"),
    }
}

/// Exact wire JSON returned by live Stalwart v1.0.0 (0.16.22) on
/// ParticipantIdentity/get (Finding 19).
/// Stalwart returns accountId, list, and notFound, but omits the mandatory state property
/// required by RFC 8620 Section 5.1 and draft-ietf-jmap-calendars Section 3.1.
const STALWART_PARTICIPANT_IDENTITY_GET_OMITTING_STATE: &str = r#"{"methodResponses":[["ParticipantIdentity/get",{"accountId":"d333333","list":[],"notFound":[]},"c0"]],"sessionState":"aa288e37"}"#;

const STALWART_PARTICIPANT_IDENTITY_GET_WITH_LIST_OMITTING_STATE: &str = r#"{"methodResponses":[["ParticipantIdentity/get",{"accountId":"d333333","list":[{"id":"pi1","name":"Admin PI","calendarAddress":"mailto:admin@example.internal","isDefault":true}],"notFound":[]},"c0"]],"sessionState":"aa288e37"}"#;

struct MockParticipantIdentityGetTransport {
    response_body: &'static str,
}

impl Transport for MockParticipantIdentityGetTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let body = match request.method {
            HttpMethod::Get => STALWART_AUTHENTICATED_SESSION_WITH_CALENDARS
                .as_bytes()
                .to_vec(),
            HttpMethod::Post => self.response_body.as_bytes().to_vec(),
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
fn finding_19_participant_identity_get_omits_state_tolerated() {
    let client = Client::builder()
        .transport(MockParticipantIdentityGetTransport {
            response_body: STALWART_PARTICIPANT_IDENTITY_GET_OMITTING_STATE,
        })
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");
    let identities = client
        .participant_identities(&account_id)
        .expect("participant_identities must tolerate omitted state property");
    assert!(identities.is_empty());

    // Strict RFC 8620 GetResponse<ParticipantIdentity> fails deserialization
    // on Stalwart's wire response because state is required:
    let wire_args: serde_json::Value =
        serde_json::from_str(r#"{"accountId":"d333333","list":[],"notFound":[]}"#).unwrap();
    let strict_result: Result<GetResponse<ParticipantIdentity>, _> =
        serde_json::from_value(wire_args);
    match strict_result {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("missing field") && msg.contains("state"),
                "expected missing field `state` error, got: {msg}"
            );
        }
        Ok(_) => panic!("expected strict GetResponse deserialization to fail on omitted state"),
    }
}

#[test]
fn finding_19_participant_identity_get_with_list_omits_state_tolerated() {
    let client = Client::builder()
        .transport(MockParticipantIdentityGetTransport {
            response_body: STALWART_PARTICIPANT_IDENTITY_GET_WITH_LIST_OMITTING_STATE,
        })
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");
    let identities = client
        .participant_identities(&account_id)
        .expect("participant_identities must tolerate omitted state property");
    assert_eq!(identities.len(), 1);
    let identity = &identities[0];
    assert_eq!(identity.id.as_ref(), Some(&Id::new("pi1")));
    assert_eq!(identity.name, "Admin PI");
    assert_eq!(
        identity.calendar_address.as_deref(),
        Some("mailto:admin@example.internal")
    );
    assert_eq!(identity.is_default, Some(true));
}
