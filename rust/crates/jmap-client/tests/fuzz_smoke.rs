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
        let _ = client.current_user_principal(&account_id);
        let _ = client.current_user_principal_id(&account_id);
        let _ = client.principal_query(
            &account_id,
            jmap_proto::principals::PrincipalQueryFilter::default(),
        );
        let _ = client.principal_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::principals::Principal,
        >::new(account_id.clone()));
        let _ = client.principal_query_changes(&jmap_proto::methods::QueryChangesRequest::new(
            account_id.clone(),
            jmap_proto::State::new("0"),
        ));
        let _ = client.share_notification_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::principals::ShareNotification,
        >::new(account_id.clone()));
        let _ = client.mailbox_update(
            &account_id,
            &Id::new("m1"),
            serde_json::json!({"name": "renamed"}),
        );
        let _ = client.mailbox_set(
            &jmap_proto::methods::SetRequest::<jmap_proto::mail::Mailbox>::new(account_id.clone()),
        );
        let _ = client.vacation_response_get_with_state(&account_id);
        let _ =
            client.vacation_response_update(&account_id, serde_json::json!({"isEnabled": true}));
        let _ = client.vacation_response_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::mail::VacationResponse,
        >::new(account_id.clone()));
        let _ = client.email_state(&account_id);
        let _ = client.email_query(
            &account_id,
            jmap_proto::mail::EmailQueryFilter::default(),
            None,
            None,
            0,
        );
        let _ = client.email_query_every_id(
            &account_id,
            jmap_proto::mail::EmailQueryFilter::default(),
            None,
        );
        let _ = client.email_query_then_get(
            &account_id,
            jmap_proto::mail::EmailQueryFilter::default(),
            None,
            None,
        );
        let _ = client.email_set(
            &jmap_proto::methods::SetRequest::<jmap_proto::mail::Email>::new(account_id.clone()),
        );
        let _ = client.email_update(
            &account_id,
            &Id::new("e1"),
            serde_json::json!({"keywords": {"$seen": true}}),
        );
        let _ = client.email_destroy(&account_id, &Id::new("e1"));
        let _ = client.email_import(
            &account_id,
            &jmap_proto::mail::EmailImport::new(Id::new("b1"), Id::new("m1")),
        );
        let _ = client.email_parse(&jmap_proto::mail::EmailParseRequest::new(
            account_id.clone(),
            [Id::new("b1")],
        ));
        let _ = client.email_submission_get(&account_id, [Id::new("sub1")]);
        let _ = client.email_submission_query(
            &account_id,
            jmap_proto::mail::EmailSubmissionQueryFilter::new(),
        );
        let _ = client.cancel_email_submission(&account_id, &Id::new("sub1"));
        let _ = client.sieve_script_create(
            &account_id,
            &jmap_proto::sieve::SieveScript::new("script_name", Id::new("b1")),
        );
        let _ = client.sieve_script_activate(&account_id, &Id::new("s1"));
        let _ = client.sieve_script_deactivate(&account_id);
        let _ = client.sieve_script_destroy(&account_id, &Id::new("s1"));
        let _ = client.calendar_update(
            &account_id,
            &Id::new("c1"),
            serde_json::json!({"name": "renamed"}),
        );
        let _ = client.calendar_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::calendars::Calendar,
        >::new(account_id.clone()));
        let _ = client.calendar_event_state(&account_id);
        let _ = client.event_update(
            &account_id,
            &Id::new("ev1"),
            serde_json::json!({"title": "updated"}),
        );
        let _ = client.event_set(&jmap_proto::calendars::CalendarEventSetRequest::new(
            jmap_proto::methods::SetRequest::new(account_id.clone()),
        ));
        let _ = client.event_query(
            &account_id,
            jmap_proto::calendars::CalendarEventQueryFilter::default(),
        );
        let _ = client.event_query_every_id(
            &account_id,
            jmap_proto::calendars::CalendarEventQueryFilter::default(),
        );
        let _ = client.participant_identity_create(
            &account_id,
            &jmap_proto::calendars::ParticipantIdentity::new("pi"),
        );
        let _ = client.participant_identity_update(
            &account_id,
            &Id::new("pi1"),
            serde_json::json!({"name": "pi"}),
        );
        let _ = client.participant_identity_destroy(&account_id, &Id::new("pi1"));
        let _ = client.participant_identity_set_default(&account_id, &Id::new("pi1"));
        let _ = client.contact_update(
            &account_id,
            &Id::new("card1"),
            serde_json::json!({"notes": "note"}),
        );
        let _ = client.contact_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::contacts::ContactCard,
        >::new(account_id.clone()));
        let _ = client.contact_state(&account_id);
        let _ =
            client.address_book_create(&account_id, &jmap_proto::contacts::AddressBook::new("ab"));
        let _ = client.address_book_destroy(&account_id, &Id::new("ab1"));
        let _ = client.address_book_update(
            &account_id,
            &Id::new("ab1"),
            serde_json::json!({"name": "ab_renamed"}),
        );
        let _ = client.address_book_set(&jmap_proto::methods::SetRequest::<
            jmap_proto::contacts::AddressBook,
        >::new(account_id.clone()));
        let _ = client.contact_query(
            &account_id,
            jmap_proto::contacts::ContactCardQueryFilter::default(),
        );
        let _ = client.contact_query_every_id(
            &account_id,
            jmap_proto::contacts::ContactCardQueryFilter::default(),
        );
        let _ = client.all_changes(&account_id, "Mailbox", &jmap_proto::State::new("0"));
        let _ = client.all_changes(&account_id, "Email", &jmap_proto::State::new("0"));
        let _ = client.blob_upload(&jmap_proto::blob::BlobUploadRequest::new(
            account_id.clone(),
        ));
        let _ = client.snooze_email(
            &account_id,
            &Id::new("e1"),
            &Id::new("m_snooze"),
            &jmap_proto::mail::SnoozeDetails::new(jmap_proto::UtcDate::new("2026-12-01T00:00:00Z")),
        );
        let _ = client.session();
        let _ = client.session_snapshot();
        let _ = client.rebase_note();
        let _ = client.rebase_origin();
        let _ = client.session_url();
        let _ = client.is_anonymous();
        let _ = client.authorization_header();
        let _ = client.id_budget();
        let _ = client.next_call_id();
        let _ = client.takes_calls_in_one_request(10);
        let _ = client.primary_account("urn:ietf:params:jmap:mail");
        let _ = client.primary_account("urn:ietf:params:jmap:contacts");
        let _ = client.primary_account("urn:ietf:params:jmap:calendars");
        let _ = client.primary_account("urn:ietf:params:jmap:principals");
        let _ = client.calendar_get(&account_id, &[Id::new("cal1")]);
        let _ = client.calendar_set_custom(&jmap_proto::calendars::CalendarSetRequest::new(
            account_id.clone(),
        ));
        let _ = client.sieve_scripts_with_state(&account_id);
        let _ = client.sieve_script_validate(&account_id, Id::new("b1"));
        let _ = client.sieve_script_validate_content(&account_id, "require [\"fileinto\"];");
        let _ = client.sieve_script_set(&jmap_proto::sieve::SieveScriptSetRequest::new(
            jmap_proto::methods::SetRequest::new(account_id.clone()),
        ));
        let _ = client.sieve_script_query_request(&jmap_proto::methods::QueryRequest::new(
            account_id.clone(),
        ));
        let _ = client.get_chunked::<jmap_proto::mail::Mailbox>(
            &["urn:ietf:params:jmap:mail"],
            "Mailbox",
            &account_id,
            &[Id::new("m1")],
            None,
            |m| m.id.as_ref(),
        );
        let _ = client.single_call(
            &["urn:ietf:params:jmap:mail"],
            "Mailbox/get",
            &serde_json::json!({"accountId": "acc1", "ids": ["m1"]}),
        );
        let _ = Client::is_method_idempotent("Mailbox/get");
        let _ = Client::is_method_idempotent("Email/set");
        if let Ok(req) = jmap_proto::request::Request::new(["urn:ietf:params:jmap:core"]).call(
            "Mailbox/get",
            &serde_json::json!({"accountId": "acc1", "ids": ["m1"]}),
            "c0",
        ) {
            let _ = Client::is_request_idempotent(&req);
        }
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
    if let Ok(client) = Client::builder()
        .transport(transport)
        .connect("https://mail.example.internal", Credentials::none())
    {
        let _ = client.session();
        let _ = client.session_snapshot();
        let _ = client.rebase_note();
        let _ = client.rebase_origin();
        let _ = client.session_url();
        let _ = client.is_anonymous();
        let _ = client.authorization_header();
        let _ = client.id_budget();
        let _ = client.next_call_id();
        let _ = client.takes_calls_in_one_request(10);
        let _ = client.primary_account("urn:ietf:params:jmap:mail");
        let _ = client.is_session_stale();
        let _ = client.last_session_state();
        let _ = client.refresh_session();
    }

    let auth_transport = MockSessionTransport {
        session_body: body.to_vec(),
    };
    if let Ok(client) = Client::builder()
        .transport(auth_transport)
        .rebase_urls_to_origin(true)
        .connect_timeout(std::time::Duration::from_millis(50))
        .read_timeout(std::time::Duration::from_millis(50))
        .max_rate_limit_retries(1)
        .max_retry_after(std::time::Duration::from_millis(100))
        .id_budget(jmap_client::limits::IdBudget::per_sync())
        .connect(
            "https://mail.example.internal",
            Credentials::basic("user", "pass"),
        )
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
        parse_chunked_stream, parse_chunked_stream_with_last_id, parse_response_head, parse_stream,
        parse_stream_with_last_id, parse_url, safe_header_name, safe_header_value,
    };
    use jmap_client::transport::CancelFlag;
    use jmap_client::url::{origin, origin_of, same_origin, split_http_scheme};

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
                for (account_id, type_states) in &state_change.changed {
                    let _ = account_id.as_str();
                    for (type_name, state) in type_states {
                        let _ = type_name.as_str();
                        let _ = state.as_str();
                    }
                }
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
                for (account_id, type_states) in &state_change.changed {
                    let _ = account_id.as_str();
                    for (type_name, state) in type_states {
                        let _ = type_name.as_str();
                        let _ = state.as_str();
                    }
                }
            }
        }

        let last_id_slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        let (tx_id, rx_id) = std::sync::mpsc::channel();
        let _ = parse_stream_with_last_id(
            std::io::BufReader::new(*input),
            &cancel,
            &tx_id,
            &last_id_slot,
        );
        while rx_id.try_recv().is_ok() {}
        let _ = format!("{:?}", *last_id_slot.lock().unwrap());

        let last_id_chunk = std::sync::Arc::new(std::sync::Mutex::new(Some("seed-id".to_string())));
        let (tx_cid, rx_cid) = std::sync::mpsc::channel();
        let _ = parse_chunked_stream_with_last_id(
            std::io::BufReader::new(*input),
            &cancel,
            &tx_cid,
            &last_id_chunk,
        );
        while rx_cid.try_recv().is_ok() {}
        let _ = format!("{:?}", *last_id_chunk.lock().unwrap());

        let (tx_small, rx_small) = std::sync::mpsc::channel();
        let _ = parse_stream(
            std::io::BufReader::with_capacity(1, *input),
            &cancel,
            &tx_small,
        );
        let _ = parse_chunked_stream(
            std::io::BufReader::with_capacity(1, *input),
            &cancel,
            &tx_small,
        );
        let _ = parse_stream(
            std::io::BufReader::with_capacity(7, *input),
            &cancel,
            &tx_small,
        );
        let _ = parse_chunked_stream(
            std::io::BufReader::with_capacity(7, *input),
            &cancel,
            &tx_small,
        );
        while rx_small.try_recv().is_ok() {}

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
        let _ = parse_response_head(std::io::BufReader::with_capacity(1, *input));
        let _ = parse_response_head(std::io::BufReader::with_capacity(7, *input));

        let timeouts = EventSourceTimeouts::default();
        let _ = format!("{timeouts:?}");
        let cloned = timeouts;
        let _ = timeouts == cloned;

        let custom_timeouts = EventSourceTimeouts {
            connect: std::time::Duration::from_millis(input.len() as u64),
            read_head: std::time::Duration::from_secs(1),
            stream_read: std::time::Duration::from_secs(2),
        };
        let _ = format!("{custom_timeouts:?}");
        let _ = timeouts == custom_timeouts;

        let empty_headers = SharedHeaders::new(vec![]);
        let _ = format!("{empty_headers:?}");
        let cloned_empty = empty_headers.clone();
        let _ = format!("{cloned_empty:?}");
        empty_headers.set(vec![]);

        if let Ok(s) = std::str::from_utf8(input) {
            let _ = expand_url(s, &[], false, 0);
            let _ = expand_url(s, &["Email", "Mailbox"], false, 30);
            let _ = expand_url(s, &[s], true, 0);
            let _ = expand_url(s, &[s, "ContactCard", "CalendarEvent"], true, 300);
            let _ = expand_url(s, &[ALL_TYPES], false, u32::MAX);
            let _ = expand_url(s, &["*"], true, 60);
            let _ = expand_url(s, &[s, "custom/type", "a,b"], false, 15);
            let _ = expand_url(
                "https://example.com/eventsource/{types}/{closeafter}/{ping}",
                &[s],
                false,
                10,
            );
            let _ = expand_url(
                "https://example.com/eventsource?types={types}&ping={ping}",
                &[s, "Thread"],
                true,
                120,
            );
            let _ = expand_url("https://example.com/eventsource?custom=1", &[s], true, 0);
            let _ = expand_url(
                "https://example.com/eventsource/{types}",
                &[s, "Email,Mailbox", "*", "a/b", "c?d#e"],
                true,
                60,
            );

            let _ = origin(s);
            let _ = origin_of(s);
            let _ = split_http_scheme(s);
            let _ = same_origin(s, s);
            let _ = same_origin(s, "https://example.com");
            let _ = same_origin("https://example.com", s);
            let _ = same_origin(s, "http://example.com:80");
            let _ = same_origin(s, "https://example.com:443");
            let _ = same_origin(s, "http://localhost");
            let _ = same_origin(s, "https://[::1]:8443");
            let _ = same_origin(s, "http://[::1]:80/");
            let _ = same_origin(s, "https://example.org:443/path?query#frag");

            let headers = SharedHeaders::new(vec![("X-Fuzz".to_string(), s.to_string())]);
            let _ = format!("{headers:?}");
            let cloned_headers = headers.clone();
            let _ = format!("{cloned_headers:?}");
            headers.set(vec![(s.to_string(), "val".to_string())]);
            headers.set(vec![("Last-Event-ID".to_string(), s.to_string())]);

            let multi_headers = SharedHeaders::new(vec![
                ("Header-1".to_string(), s.to_string()),
                ("Header-2".to_string(), "val".to_string()),
            ]);
            let _ = format!("{multi_headers:?}");
            let cloned_multi = multi_headers.clone();
            let _ = format!("{cloned_multi:?}");
            multi_headers.set(vec![(s.to_string(), s.to_string())]);
            multi_headers.set(vec![
                ("Authorization".to_string(), format!("Bearer {s}")),
                ("Last-Event-ID".to_string(), s.to_string()),
            ]);

            let reconnected = EventSourceItem::Reconnected {
                last_event_id: Some(s.to_string()),
                reconnect_count: input.len() as u64,
            };
            let _ = format!("{reconnected:?}");
            let cloned_recon = reconnected.clone();
            let _ = reconnected == cloned_recon;

            let reconnected_none = EventSourceItem::Reconnected {
                last_event_id: None,
                reconnect_count: 0,
            };
            let _ = reconnected == reconnected_none;

            let _ = safe_header_name(s);
            let _ = safe_header_value(s);
            if let Some(parts) = parse_url(s) {
                let _ = format!("{parts:?}");
                let cloned_parts = parts.clone();
                assert_eq!(parts, cloned_parts);
                let _ = parts.tls;
                let _ = parts.authority.as_str();
                let _ = parts.dial.as_str();
                let _ = parts.host.as_str();
                let _ = parts.path_and_query.as_str();
            }
            for url_tmpl in [
                format!("http://{s}/eventsource"),
                format!("https://{s}/eventsource"),
                format!("http://example.com:80/{s}"),
                format!("https://[::1]:8443/{s}"),
            ] {
                let _ = parse_url(&url_tmpl);
            }
        }

        let lossy = String::from_utf8_lossy(input);
        let _ = safe_header_name(&lossy);
        let _ = safe_header_value(&lossy);
        let _ = parse_url(&lossy);
    }
}

#[cfg(feature = "websocket")]
#[test]
fn fuzz_websocket_frames_smoke() {
    use jmap_client::websocket::{
        DEFAULT_MAX_FRAME_SIZE, DEFAULT_MAX_MESSAGE_SIZE, InboundFrameAction, WebSocketLimits,
        WebSocketPushTimeouts, WebSocketTimeouts, parse_inbound_message, parse_inbound_text,
    };
    use jmap_proto::websocket::{
        WebSocketPushDisable, WebSocketPushEnable, WebSocketRequest, WebSocketRequestError,
        WebSocketResponse,
    };
    use tungstenite::Message;
    use tungstenite::protocol::CloseFrame;
    use tungstenite::protocol::frame::coding::CloseCode;

    let inputs: &[&[u8]] = &[
        b"",
        b"not json",
        b"{incomplete",
        b"12345",
        b"[]",
        br#"{"hello":"world"}"#,
        br#"{"@type":"UnknownType","foo":"bar"}"#,
        br#"{"@type":"Response","requestId":"r1","methodResponses":[],"sessionState":"s1"}"#,
        br#"{"@type":"Response","id":"r2","methodResponses":[["Core/echo",{},"c1"]],"sessionState":"s2"}"#,
        br#"{"@type":"RequestError","requestId":"r3","type":"urn:ietf:params:jmap:error:notJSON","status":400,"detail":"bad json"}"#,
        br#"{"@type":"StateChange","changed":{"acc1":{"Email":"s1"}},"pushState":"p1"}"#,
        br#"{"@type":"StateChange","changed":{}}"#,
        br#"{"@type":"WebSocketPushEnable","dataTypes":["Email","Mailbox"]}"#,
        br#"{"@type":"WebSocketPushDisable"}"#,
        br#"{"@type":"Request","id":"req-1","using":["urn:ietf:params:jmap:core"],"methodCalls":[["Core/echo",{},"call-1"]]}"#,
        b"\x00\x01\x02\x03\xff\xfe\xfd",
        &[0x7b; 1024],
        b"{\"@type\":\"Response\",\"requestId\":\"r1\"}\n{\"@type\":\"Unknown\"}\nbogus\n",
    ];

    for input in inputs {
        if let Ok(s) = std::str::from_utf8(input) {
            let act = parse_inbound_text(s);
            let _ = format!("{act:?}");
            assert_eq!(act, act.clone());

            let _ = serde_json::from_str::<WebSocketResponse>(s);
            let _ = serde_json::from_str::<WebSocketRequestError>(s);
            let _ = serde_json::from_str::<WebSocketRequest>(s);
            let _ = serde_json::from_str::<WebSocketPushEnable>(s);
            let _ = serde_json::from_str::<WebSocketPushDisable>(s);
        }

        let lossy = String::from_utf8_lossy(input);
        let act_lossy = parse_inbound_text(&lossy);
        let _ = format!("{act_lossy:?}");

        match &act_lossy {
            InboundFrameAction::Response { id, response } => {
                let _ = id.as_deref();
                let _ = response.id.as_deref();
                let _ = response.session_state.as_ref();
                let _ = response.created_ids.as_ref();
                let _ = response.responses_for("c1").count();
                for r in &response.method_responses {
                    let _ = r.name.as_str();
                    let _ = r.call_id.as_str();
                }
            }
            InboundFrameAction::RequestError { id, error } => {
                let _ = id.as_deref();
                let _ = error.id.as_deref();
                let _ = &error.error_type;
                let _ = error.status;
                let _ = error.detail.as_deref();
            }
            InboundFrameAction::StateChange {
                state_change,
                push_state,
            } => {
                let _ = push_state.as_deref();
                let _ = state_change.changed.len();
                for (acc, map) in &state_change.changed {
                    let _ = acc.as_str();
                    for (typ, st) in map {
                        let _ = typ.as_str();
                        let _ = st.as_str();
                    }
                }
            }
            InboundFrameAction::Close(reason) => {
                let _ = reason.as_deref();
            }
            InboundFrameAction::Ping(p) | InboundFrameAction::Pong(p) => {
                let _ = p.len();
            }
            InboundFrameAction::Ignored => {}
        }

        let text_msg = Message::Text(lossy.into_owned().into());
        let _ = parse_inbound_message(&text_msg);

        let bin_msg = Message::Binary(input.to_vec().into());
        assert_eq!(parse_inbound_message(&bin_msg), InboundFrameAction::Ignored);

        let ping_msg = Message::Ping(input.to_vec().into());
        let _ = parse_inbound_message(&ping_msg);

        let pong_msg = Message::Pong(input.to_vec().into());
        let _ = parse_inbound_message(&pong_msg);

        let close_some = Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: String::from_utf8_lossy(&input[..input.len().min(125)])
                .into_owned()
                .into(),
        }));
        let _ = parse_inbound_message(&close_some);

        let close_none = Message::Close(None);
        assert_eq!(
            parse_inbound_message(&close_none),
            InboundFrameAction::Close(None)
        );

        let default_limits = WebSocketLimits::default();
        assert_eq!(default_limits.max_message_size, DEFAULT_MAX_MESSAGE_SIZE);
        assert_eq!(default_limits.max_frame_size, DEFAULT_MAX_FRAME_SIZE);

        let custom_limits = WebSocketLimits {
            max_message_size: input.len(),
            max_frame_size: input.len() / 2,
        };
        let _ = format!("{custom_limits:?}");
        let cloned_limits = custom_limits;
        assert_eq!(custom_limits, cloned_limits);

        let zero_limits = WebSocketLimits {
            max_message_size: 0,
            max_frame_size: 0,
        };
        let _ = format!("{zero_limits:?}");

        let timeouts = WebSocketTimeouts {
            connect: std::time::Duration::from_millis(input.len() as u64),
            request: std::time::Duration::from_millis(input.len() as u64 * 2),
        };
        let _ = format!("{timeouts:?}");
        let cloned_timeouts = timeouts;
        assert_eq!(timeouts, cloned_timeouts);

        let push_timeouts = WebSocketPushTimeouts {
            connect: std::time::Duration::from_millis(input.len() as u64),
            ping_interval: std::time::Duration::from_millis(500),
            ping_timeout: std::time::Duration::from_millis(100),
            initial_backoff: std::time::Duration::from_millis(50),
            max_backoff: std::time::Duration::from_millis(1000),
        };
        let _ = format!("{push_timeouts:?}");
        let cloned_push = push_timeouts;
        assert_eq!(push_timeouts, cloned_push);

        let default_push = WebSocketPushTimeouts::default();
        let _ = format!("{default_push:?}");
        let cloned_default = default_push;
        assert_eq!(default_push, cloned_default);
    }
}
