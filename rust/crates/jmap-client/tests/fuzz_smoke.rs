// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smoke and regression tests for jmap-client response handling fuzzer.
//!
//! Asserts that feeding arbitrary or malformed response bodies through a fake
//! transport yields standard client errors without panicking.

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials};
use jmap_proto::Id;

const MINIMAL_AUTHENTICATED_SESSION: &str = r#"{"capabilities":{"urn:ietf:params:jmap:core":{"maxSizeUpload":50000000,"maxConcurrentUpload":4,"maxSizeRequest":10000000,"maxConcurrentRequests":4,"maxCallsInRequest":16,"maxObjectsInGet":500,"maxObjectsInSet":500,"collationAlgorithms":["i;ascii-numeric","i;ascii-casemap","i;unicode-casemap"]},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:contacts":{},"urn:ietf:params:jmap:calendars":{},"urn:ietf:params:jmap:principals":{},"urn:ietf:params:jmap:sieve":{},"urn:ietf:params:jmap:quota":{},"urn:ietf:params:jmap:blob":{}},"accounts":{"acc1":{"name":"user","isPersonal":true,"isReadOnly":false,"accountCapabilities":{"urn:ietf:params:jmap:core":{},"urn:ietf:params:jmap:mail":{},"urn:ietf:params:jmap:contacts":{},"urn:ietf:params:jmap:calendars":{},"urn:ietf:params:jmap:principals":{},"urn:ietf:params:jmap:sieve":{},"urn:ietf:params:jmap:quota":{},"urn:ietf:params:jmap:blob":{}}}},"primaryAccounts":{"urn:ietf:params:jmap:mail":"acc1","urn:ietf:params:jmap:contacts":"acc1","urn:ietf:params:jmap:calendars":"acc1","urn:ietf:params:jmap:principals":"acc1"},"username":"user","apiUrl":"https://mail.example.internal/jmap/","downloadUrl":"https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?accept={type}","uploadUrl":"https://mail.example.internal/jmap/upload/{accountId}/","eventSourceUrl":"https://mail.example.internal/jmap/eventsource/","state":"0"}"#;

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
        let _ = client.changes(&account_id, "Mailbox", &jmap_proto::State::new("0"));
        let _ = client.changes(&account_id, "Email", &jmap_proto::State::new("0"));
        let _ = client.changes(&account_id, "ContactCard", &jmap_proto::State::new("0"));
        let _ = client.changes(&account_id, "CalendarEvent", &jmap_proto::State::new("0"));
        let _ = client.download_blob(&account_id, &Id::new("b1"), "test.txt", 1024);
        let _ = client.upload_blob(&account_id, "application/octet-stream", vec![0]);
        let _ = client.echo(serde_json::json!({}));
        let _ = client.principal_get(&account_id, &[Id::new("p1")]);
        let _ = client.principals(&account_id);
        let _ = client.participant_identities(&account_id);
        let _ = client.calendar_event_notifications(&account_id);
        let _ = client.share_notifications(&account_id);
        let _ = client.vacation_response_get(&account_id);
        let _ = client.identities(&account_id);
        let _ = client.thread_get(&account_id, [Id::new("t1")]);
        let _ = client.mailbox_create(
            &account_id,
            &jmap_proto::mail::Mailbox::new("fuzz_test_folder"),
        );
        let _ = client.mailbox_destroy(&account_id, &Id::new("m1"));
        let _ = client.calendar_create(
            &account_id,
            &jmap_proto::calendars::Calendar::new("fuzz_cal"),
        );
        let _ = client.calendar_destroy(&account_id, &Id::new("c1"));
        let _ = client.contact_create(&account_id, &jmap_proto::contacts::ContactCard::new());
        let _ = client.contact_destroy(&account_id, &Id::new("card1"));
        let _ = client.event_create(&account_id, &jmap_proto::calendars::CalendarEvent::new());
        let _ = client.event_destroy(&account_id, &Id::new("ev1"));
        let _ = client.event_parse(&jmap_proto::calendars::CalendarEventParseRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.contact_card_parse(&jmap_proto::contacts::ContactCardParseRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.sieve_script_get(&account_id, &Id::new("s1"));
        let _ = client.sieve_script_active(&account_id);
        let _ = client.sieve_script_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.principal_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.share_notification_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.calendar_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.event_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.participant_identity_changes(&account_id, &jmap_proto::State::new("0"));
        let _ =
            client.calendar_event_notification_changes(&account_id, &jmap_proto::State::new("0"));
        let _ = client.events(&account_id);
        let _ = client.contact_cards(&account_id);
        let _ = client.quotas(&account_id);
        let _ = client.quota_query(&account_id, jmap_proto::quota::QuotaQueryFilter::new());
        let _ = client.blob_get(&jmap_proto::blob::BlobGetRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.blob_lookup(&jmap_proto::blob::BlobLookupRequest::new(
            account_id.clone(),
            ["Email"],
            [Id::new("b1")],
        ));
        let _ = client.blob_copy(&jmap_proto::methods::BlobCopyRequest::new(
            account_id.clone(),
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.contact_copy(&jmap_proto::methods::CopyRequest::new(
            account_id.clone(),
            account_id.clone(),
        ));
        let _ = client.contact_query_changes(&jmap_proto::methods::QueryChangesRequest::new(
            account_id.clone(),
            jmap_proto::State::new("0"),
        ));
        let _ = client.event_copy(&jmap_proto::methods::CopyRequest::new(
            account_id.clone(),
            account_id.clone(),
        ));
        let _ = client.event_query_changes(&jmap_proto::methods::QueryChangesRequest::new(
            account_id.clone(),
            jmap_proto::State::new("0"),
        ));
        let _ = client.calendar_event_notification_query(
            &account_id,
            jmap_proto::calendars::CalendarEventNotificationQueryFilter::new(),
        );
        let _ = client.calendar_event_notification_destroy(&account_id, &Id::new("notif1"));
        let _ = client.email_copy(&jmap_proto::methods::CopyRequest::new(
            account_id.clone(),
            account_id.clone(),
        ));
        let _ = client.search_snippet_get(
            &account_id,
            vec![Id::new("e1")],
            None::<jmap_proto::mail::EmailQueryFilter>,
        );
        let _ = client.share_notification_get(&account_id, &[Id::new("sn1")]);
        let _ = client.share_notification_query(
            &account_id,
            jmap_proto::principals::ShareNotificationQueryFilter::new(),
        );
        let _ = client.share_notification_query_changes(
            &jmap_proto::methods::QueryChangesRequest::new(
                account_id.clone(),
                jmap_proto::State::new("0"),
            ),
        );
        let _ = client.share_notification_destroy(&account_id, &Id::new("sn1"));
        let _ = client.sieve_scripts(&account_id);
        let _ = client.sieve_script_query(
            &account_id,
            jmap_proto::sieve::SieveScriptQueryFilter::new(),
        );
        let _ = client.get_availability(
            &account_id,
            &Id::new("p1"),
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
            false,
        );
        let _ = client.snoozed_mailbox(&account_id);
        let _ = client.is_session_stale();
        let _ = client.last_session_state();
    }
}

struct MockSessionTransport {
    session_body: Vec<u8>,
}

impl Transport for MockSessionTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: self.session_body.clone(),
            final_url: request.url.to_owned(),
        })
    }
}

fn exercise_session_connect_with_body(body: &[u8]) {
    let transport = MockSessionTransport {
        session_body: body.to_vec(),
    };
    if let Ok(mut client) = Client::builder()
        .transport(transport)
        .connect("https://mail.example.internal", Credentials::none())
    {
        let _ = client.is_session_stale();
        let _ = client.last_session_state();
        let _ = client.refresh_session();
    }
}

#[test]
fn hostile_response_bodies_never_panic() {
    let hostile_payloads: &[&[u8]] = &[
        b"",
        b"{",
        b"[",
        b"null",
        b"404 Not Found",
        b"{\"methodResponses\": null}",
        b"{\"methodResponses\": [\"invalid\"]}",
        b"{\"methodResponses\": [[\"error\", {\"type\": \"serverFail\"}, \"c0\"]]}",
        b"{\"methodResponses\": [[\"Mailbox/get\", null, \"c0\"]]}",
        b"{\"methodResponses\": [[\"Mailbox/get\", {}, \"c0\"]]}",
        b"{\"methodResponses\": [[\"Mailbox/set\", {\"created\": null, \"notCreated\": null}, \"c0\"]]}",
        b"{\"sessionState\": \"abc\"}",
        b"{\"sessionState\": null, \"methodResponses\": []}",
        b"\xFF\xFE\x00\x00",
        b"{\"parsed\": {\"b1\": [{}, {}]}}",
        b"{\"parsed\": {\"b1\": []}}",
        b"{\"parsed\": null}",
    ];

    for payload in hostile_payloads {
        exercise_client_with_body(payload);
        exercise_session_connect_with_body(payload);
    }
}

#[test]
fn hostile_eventsource_streams_never_panic() {
    use jmap_client::eventsource::{
        ALL_TYPES, EventSourceItem, EventSourceTimeouts, SharedHeaders, expand_url,
        parse_chunked_stream, parse_response_head, parse_stream,
    };
    use jmap_client::transport::CancelFlag;
    use jmap_client::url::{origin, origin_of, split_http_scheme};

    let hostile_inputs: &[&[u8]] = &[
        b"",
        b"\r\n",
        b"HTTP/1.1 200 OK\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"HTTP/1.1 500 Error\r\n\r\n",
        b"0\r\n\r\n",
        b"event: state\r\ndata: {}\r\n\r\n",
        b"event: ping\r\ndata: \r\n\r\n",
        b"event: state\r\ndata: {\"changed\": {\"acc1\": {\"Mailbox\": \"state1\"}}}\r\n\r\n",
        b"id: 123\r\nevent: state\r\ndata: not json\r\n\r\n",
        b": comment\r\n\r\n",
        b"\xFF\xFE\x00\x00\r\n\r\n",
    ];

    for input in hostile_inputs {
        let cancel = CancelFlag::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = parse_stream(std::io::BufReader::new(*input), &cancel, &tx);
        while let Ok(item) = rx.try_recv() {
            let _ = format!("{item:?}");
            let cloned = item.clone();
            let _ = item == cloned;
            if let EventSourceItem::State(ref state_change) = item {
                let _ = format!("{state_change:?}");
                let _ = &state_change.changed;
                let _ = &state_change.extra;
                let _ = &state_change.kind;
            }
        }

        let _ = parse_chunked_stream(std::io::BufReader::new(*input), &cancel, &tx);
        while let Ok(item) = rx.try_recv() {
            let _ = format!("{item:?}");
            let cloned = item.clone();
            let _ = item == cloned;
            if let EventSourceItem::State(ref state_change) = item {
                let _ = format!("{state_change:?}");
                let _ = &state_change.changed;
                let _ = &state_change.extra;
                let _ = &state_change.kind;
            }
        }

        let cancelled = CancelFlag::new();
        cancelled.cancel();
        let (tx_c, rx_c) = std::sync::mpsc::channel();
        let _ = parse_stream(std::io::BufReader::new(*input), &cancelled, &tx_c);
        let _ = parse_chunked_stream(std::io::BufReader::new(*input), &cancelled, &tx_c);
        while rx_c.try_recv().is_ok() {}

        if input.len() > 8 {
            let dyn_cancel = CancelFlag::new();
            let (tx_d, rx_d) = std::sync::mpsc::channel();
            let mid = input.len() / 2;
            let _ = parse_stream(std::io::BufReader::new(&input[..mid]), &dyn_cancel, &tx_d);
            dyn_cancel.cancel();
            let _ = parse_stream(std::io::BufReader::new(&input[mid..]), &dyn_cancel, &tx_d);
            while rx_d.try_recv().is_ok() {}
        }

        let _ = parse_response_head(std::io::BufReader::new(*input));

        let timeouts = EventSourceTimeouts::default();
        let _ = format!("{timeouts:?}");
        let _ = timeouts == timeouts;

        if let Ok(s) = std::str::from_utf8(input) {
            let _ = expand_url(s, &[], false, 0);
            let _ = expand_url(s, &["Email", "Mailbox"], false, 30);
            let _ = expand_url(s, &[s], true, 0);
            let _ = expand_url(s, &[s, "ContactCard", "CalendarEvent"], true, 300);
            let _ = expand_url(s, &[ALL_TYPES], false, u32::MAX);

            let _ = origin(s);
            let _ = origin_of(s);
            let _ = split_http_scheme(s);

            let headers = SharedHeaders::new(vec![("X-Fuzz".to_string(), s.to_string())]);
            let _ = format!("{headers:?}");
            headers.set(vec![(s.to_string(), "val".to_string())]);
        }
    }
}
