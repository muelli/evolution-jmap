// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pinned mock-level regression tests verifying that `jmap-client` tolerates
//! real-world spec deviations emitted by live Stalwart deployments
//! (STALWART-RFC-FINDINGS.md, Batch 25).

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarEventParseRequest, ParticipantIdentity};
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

/// Exact wire JSON returned by live Stalwart v1.0.0 (0.16.22) for session with
/// currentUserPrincipalId set to the account identifier (Finding 18).
const STALWART_AUTHENTICATED_SESSION_WITH_PRINCIPALS: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:principals":{}},"accounts":{"d333333":{"name":"admin","isPersonal":true,"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:principals":{"currentUserPrincipalId":"d333333"}}}},"primaryAccounts":{"urn:ietf:params:jmap:principals":"d333333"},"username":"admin","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/","state":"0"}"#;

struct MockCurrentUserPrincipalTransport;

impl Transport for MockCurrentUserPrincipalTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let body = match request.method {
            HttpMethod::Get => STALWART_AUTHENTICATED_SESSION_WITH_PRINCIPALS
                .as_bytes()
                .to_vec(),
            HttpMethod::Post => {
                let json: serde_json::Value =
                    serde_json::from_slice(request.body.unwrap_or_default())
                        .map_err(|e| TransportError::Failed(e.to_string()))?;
                let method_call = &json["methodCalls"][0];
                let method = method_call[0].as_str().unwrap_or_default();
                let args = &method_call[1];
                let tag = method_call[2].as_str().unwrap_or("c0");

                let response_json = match method {
                    "Principal/get" => {
                        let ids = args["ids"].as_array();
                        if let Some(ids) = ids {
                            if ids.len() == 1 && ids[0].as_str() == Some("d333333") {
                                // Finding 18: Stalwart returns notFound when querying currentUserPrincipalId:
                                format!(
                                    r#"{{"methodResponses":[["Principal/get",{{"accountId":"d333333","state":"n","list":[],"notFound":["d333333"]}},"{tag}"]],"sessionState":"aa288e37"}}"#
                                )
                            } else if ids.len() == 1 && ids[0].as_str() == Some("b") {
                                // True principal object on Stalwart:
                                format!(
                                    r#"{{"methodResponses":[["Principal/get",{{"accountId":"d333333","state":"n","list":[{{"id":"b","type":"individual","name":"admin@example.internal","description":"System administrator","email":"admin@example.internal"}}],"notFound":[]}},"{tag}"]],"sessionState":"aa288e37"}}"#
                                )
                            } else {
                                format!(
                                    r#"{{"methodResponses":[["Principal/get",{{"accountId":"d333333","state":"n","list":[],"notFound":["unknown_id"]}},"{tag}"]],"sessionState":"aa288e37"}}"#
                                )
                            }
                        } else {
                            format!(
                                r#"{{"methodResponses":[["Principal/get",{{"accountId":"d333333","state":"n","list":[{{"id":"b","type":"individual","name":"admin@example.internal","description":"System administrator","email":"admin@example.internal"}}],"notFound":[]}},"{tag}"]],"sessionState":"aa288e37"}}"#
                            )
                        }
                    }
                    "Principal/query" => {
                        format!(
                            r#"{{"methodResponses":[["Principal/query",{{"accountId":"d333333","queryState":"n","canCalculateChanges":true,"position":0,"ids":["b"]}},"{tag}"]],"sessionState":"aa288e37"}}"#
                        )
                    }
                    _ => {
                        format!(
                            r#"{{"methodResponses":[["error",{{"type":"unknownMethod"}},"{tag}"]],"sessionState":"aa288e37"}}"#
                        )
                    }
                };
                response_json.into_bytes()
            }
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
fn finding_18_current_user_principal_resolves_past_account_id_mismatch() {
    let client = Client::builder()
        .transport(MockCurrentUserPrincipalTransport)
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");

    // 1. Session advertises currentUserPrincipalId as "d333333" (account id):
    let advertised = client
        .session()
        .accounts
        .get(&account_id)
        .and_then(|acct| {
            acct.account_capabilities
                .get(session::CAPABILITY_PRINCIPALS)
        })
        .and_then(|cap| cap.get("currentUserPrincipalId"))
        .and_then(|v| v.as_str());
    assert_eq!(advertised, Some("d333333"));

    // 2. Client resolves the true principal object ("b") rather than failing:
    let principal = client
        .current_user_principal(&account_id)
        .expect("current_user_principal must succeed")
        .expect("principal must be found");
    assert_eq!(principal.id.as_ref(), Some(&Id::new("b")));
    assert_eq!(principal.name, "admin@example.internal");
    assert_eq!(principal.email.as_deref(), Some("admin@example.internal"));

    // 3. Client resolves the true principal id:
    let principal_id = client
        .current_user_principal_id(&account_id)
        .expect("current_user_principal_id must succeed")
        .expect("principal id must be found");
    assert_eq!(principal_id, Id::new("b"));
}

#[test]
fn finding_18_principal_get_tolerates_account_id_as_principal_id() {
    let client = Client::builder()
        .transport(MockCurrentUserPrincipalTransport)
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");

    // Querying with the advertised account ID "d333333" resolves the true principal:
    let list = client
        .principal_get(&account_id, &[Id::new("d333333")])
        .expect("principal_get must succeed");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id.as_ref(), Some(&Id::new("b")));

    // Querying with the real principal ID "b" succeeds directly:
    let list_b = client
        .principal_get(&account_id, &[Id::new("b")])
        .expect("principal_get must succeed");
    assert_eq!(list_b.len(), 1);
    assert_eq!(list_b[0].id.as_ref(), Some(&Id::new("b")));

    // Querying with an unrelated unknown ID returns empty without fallback:
    let list_unknown = client
        .principal_get(&account_id, &[Id::new("unknown_id")])
        .expect("principal_get must succeed");
    assert!(list_unknown.is_empty());
}

/// Exact wire JSON returned by live Stalwart v1.0.0 (0.16.22) on
/// CalendarEvent/set create when timeZones property is supplied (Finding 13).
/// Stalwart rejects the call with invalidProperties: ["timeZones"].
const STALWART_CALENDAR_EVENT_SET_TIMEZONES_REJECTED: &str = r#"{"methodResponses":[["CalendarEvent/set",{"accountId":"d333333","oldState":"spq","newState":"spq","notCreated":{"new":{"type":"invalidProperties","description":"Invalid property.","properties":["timeZones"]}}},"c0"]],"sessionState":"aa288e37"}"#;

/// Exact wire JSON returned by live Stalwart v1.0.0 (0.16.22) on
/// CalendarEvent/parse for an uploaded iCalendar blob containing a custom VTIMEZONE.
/// Stalwart represents the custom timezone under iCalendar convertedProperties
/// rather than RFC 8984 Section 4.7.2 timeZones.
const STALWART_CALENDAR_EVENT_PARSE_RESPONSE: &str = r#"{"methodResponses":[["CalendarEvent/parse",{"accountId":"d333333","parsed":{"blob1":[{"@type":"Event","iCalendar":{"convertedProperties":{"start":{"parameters":{"tzid":"/custom/zone1"}}},"name":"vevent"},"updated":"2026-10-05T00:00:00Z","title":"Test Event","start":"2026-10-05T12:00:00","uid":"test-tz-1"}]}},"c0"]],"sessionState":"aa288e37"}"#;

const CONFORMANT_CALENDAR_EVENT_PARSE_RESPONSE_WITH_TIMEZONES: &str = r#"{"methodResponses":[["CalendarEvent/parse",{"accountId":"d333333","parsed":{"blob1":[{"@type":"Event","title":"Test Event","start":"2026-10-05T12:00:00","uid":"test-tz-1","timeZones":{"/custom/zone1":{"@type":"TimeZone","timeZoneId":"/custom/zone1"}}}]}},"c0"]],"sessionState":"aa288e37"}"#;

const STALWART_AUTHENTICATED_SESSION_WITH_CALENDARS_AND_PARSE: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:calendars":{},"urn:ietf:params:jmap:calendars:parse":{}},"accounts":{"d333333":{"name":"admin","isPersonal":true,"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:calendars":{},"urn:ietf:params:jmap:calendars:parse":{}}}},"primaryAccounts":{"urn:ietf:params:jmap:calendars":"d333333","urn:ietf:params:jmap:calendars:parse":"d333333"},"username":"admin","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/","state":"0"}"#;

struct MockCalendarEventTransport {
    session_body: &'static str,
    response_body: &'static str,
}

impl Transport for MockCalendarEventTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let body = match request.method {
            HttpMethod::Get => self.session_body.as_bytes().to_vec(),
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
fn finding_13_calendar_event_set_rejects_timezones_invalid_properties() {
    let client = Client::builder()
        .transport(MockCalendarEventTransport {
            session_body: STALWART_AUTHENTICATED_SESSION_WITH_CALENDARS,
            response_body: STALWART_CALENDAR_EVENT_SET_TIMEZONES_REJECTED,
        })
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");
    let mut event = CalendarEvent::simple(
        Id::new("b"),
        "Probe Event With TimeZones",
        "2026-10-05T12:00:00",
        "PT1H",
    );
    let mut tz_map = std::collections::BTreeMap::new();
    tz_map.insert(
        "/custom/zone1".to_string(),
        serde_json::json!({
            "@type": "TimeZone",
            "timeZoneId": "/custom/zone1"
        }),
    );
    event.time_zones = Some(tz_map);

    let result = client.event_create(&account_id, &event);
    match result {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "invalidProperties");
            assert_eq!(err.description.as_deref(), Some("Invalid property."));
            assert_eq!(
                err.properties.as_ref(),
                Some(&vec!["timeZones".to_string()])
            );
        }
        other => panic!("expected Error::Set with invalidProperties, got {other:?}"),
    }
}

#[test]
fn finding_13_calendar_event_parse_tolerates_stalwart_custom_timezone_representation() {
    let client = Client::builder()
        .transport(MockCalendarEventTransport {
            session_body: STALWART_AUTHENTICATED_SESSION_WITH_CALENDARS_AND_PARSE,
            response_body: STALWART_CALENDAR_EVENT_PARSE_RESPONSE,
        })
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");
    let request = CalendarEventParseRequest::new(account_id.clone(), vec![Id::new("blob1")]);
    let response = client
        .event_parse(&request)
        .expect("event_parse must parse Stalwart wire response");

    assert_eq!(response.account_id, account_id);
    let parsed_map = response.parsed.expect("parsed map must be present");
    let event = parsed_map
        .get(&Id::new("blob1"))
        .expect("event for blob1 must be present");

    assert_eq!(event.title.as_deref(), Some("Test Event"));
    assert_eq!(event.start.as_deref(), Some("2026-10-05T12:00:00"));
    assert_eq!(event.uid.as_deref(), Some("test-tz-1"));
    assert_eq!(event.updated.as_deref(), Some("2026-10-05T00:00:00Z"));

    // Stalwart does not emit standard timeZones property, mapping it to convertedProperties:
    assert!(event.time_zones.is_none());
    assert!(
        event.extra.contains_key("iCalendar"),
        "iCalendar convertedProperties must be preserved in event extra map"
    );
}

#[test]
fn finding_13_calendar_event_parse_tolerates_standard_timezones() {
    let client = Client::builder()
        .transport(MockCalendarEventTransport {
            session_body: STALWART_AUTHENTICATED_SESSION_WITH_CALENDARS_AND_PARSE,
            response_body: CONFORMANT_CALENDAR_EVENT_PARSE_RESPONSE_WITH_TIMEZONES,
        })
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds");

    let account_id = Id::new("d333333");
    let request = CalendarEventParseRequest::new(account_id.clone(), vec![Id::new("blob1")]);
    let response = client
        .event_parse(&request)
        .expect("event_parse must parse response with standard timeZones");

    let parsed_map = response.parsed.expect("parsed map must be present");
    let event = &parsed_map[&Id::new("blob1")];
    assert_eq!(event.title.as_deref(), Some("Test Event"));
    let time_zones = event
        .time_zones
        .as_ref()
        .expect("time_zones must be populated when returned by server");
    assert!(time_zones.contains_key("/custom/zone1"));
}
