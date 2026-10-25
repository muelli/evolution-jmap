// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targeted mutation test coverage for jmap-client/src/calendars.rs:
//! - Calendar CRUD error handling and set_failure paths
//! - CalendarEvent get chunking boundaries (maxObjectsInGet zero vs positive)
//! - CalendarEvent destroy/update error handling
//! - CalendarEvent parse array unwrapping and multi-element demoting
//! - CalendarEventNotification get, query, and destroy (including Finding 20 tolerance)
//! - ParticipantIdentity get, create, update, destroy, and set_default

use std::sync::Arc;
use std::sync::Mutex;

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use jmap_proto::Id;
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventNotificationQueryFilter, CalendarEventParseRequest,
    ParticipantIdentity,
};
use serde_json::{Value, json};

// =========================================================================
// Custom Transport for Scripted Calendar Wire Responses
// =========================================================================

struct ScriptedTransport {
    handler: Arc<dyn Fn(&HttpRequest<'_>) -> HttpResponse + Send + Sync>,
}

impl Transport for ScriptedTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        Ok((self.handler)(&req))
    }
}

fn client_with_handler(
    handler: impl Fn(&HttpRequest<'_>) -> HttpResponse + Send + Sync + 'static,
) -> Client {
    Client::builder()
        .transport(ScriptedTransport {
            handler: Arc::new(handler),
        })
        .connect("https://mail.example.internal", Credentials::none())
        .expect("Client::connect succeeds")
}

fn default_session_json(max_objects_in_get: Option<usize>) -> Value {
    let mut core_cap = json!({
        "maxSizeUpload": 50000000,
        "maxConcurrentUpload": 4,
        "maxSizeRequest": 10000000,
        "maxConcurrentRequests": 4,
        "maxCallsInRequest": 16,
        "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
    });
    if let Some(limit) = max_objects_in_get {
        core_cap["maxObjectsInGet"] = json!(limit);
    }

    json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": core_cap,
            "urn:ietf:params:jmap:calendars": {}
        },
        "accounts": {
            "acc1": {
                "name": "alice@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:core": {},
                    "urn:ietf:params:jmap:calendars": {}
                }
            }
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:core": "acc1",
            "urn:ietf:params:jmap:calendars": "acc1"
        },
        "username": "alice@example.com",
        "apiUrl": "https://mail.example.internal/jmap/api",
        "downloadUrl": "https://mail.example.internal/jmap/download/{blobId}",
        "uploadUrl": "https://mail.example.internal/jmap/upload",
        "eventSourceUrl": "https://mail.example.internal/jmap/events",
        "state": "session-state-1"
    })
}

// =========================================================================
// Tests for Calendar CRUD and set_failure error paths
// =========================================================================

#[test]
fn calendars_lists_calendars() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Calendar/get", {
                    "accountId": "acc1",
                    "state": "cal-s1",
                    "list": [
                        {
                            "id": "CAL-work",
                            "name": "Work Calendar",
                            "color": "#ff0000"
                        }
                    ],
                    "notFound": []
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let cals = client.calendars(&account_id).unwrap();
    assert_eq!(cals.len(), 1);
    assert_eq!(cals[0].id.as_ref(), Some(&Id::new("CAL-work")));
    assert_eq!(cals[0].name, "Work Calendar");
    assert_eq!(cals[0].color.as_deref(), Some("#ff0000"));
}

#[test]
fn calendar_create_success() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Calendar/set", {
                    "accountId": "acc1",
                    "oldState": "s1",
                    "newState": "s2",
                    "created": {
                        "new": {
                            "id": "CAL-created-1",
                            "name": "Project Calendar",
                            "color": "#0000ff"
                        }
                    }
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let cal = Calendar::new("Project Calendar").with_color("#0000ff");
    let created = client.calendar_create(&account_id, &cal).unwrap();
    assert_eq!(created.id.as_ref(), Some(&Id::new("CAL-created-1")));
    assert_eq!(created.name, "Project Calendar");
    assert_eq!(created.color.as_deref(), Some("#0000ff"));
}

#[test]
fn calendar_destroy_and_update_fail_when_server_reports_not_destroyed_or_not_updated() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let args = &body_val["methodCalls"][0][1];

        let resp = if args.get("destroy").is_some() {
            json!({
                "methodResponses": [
                    ["Calendar/set", {
                        "accountId": "acc1",
                        "oldState": "s1",
                        "newState": "s1",
                        "notDestroyed": {
                            "CAL1": {
                                "type": "cannotDestroyDefault",
                                "description": "default calendar cannot be destroyed"
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["Calendar/set", {
                        "accountId": "acc1",
                        "oldState": "s1",
                        "newState": "s1",
                        "notUpdated": {
                            "CAL1": {
                                "type": "forbidden",
                                "description": "permission denied"
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let cal_id = Id::new("CAL1");

    match client.calendar_destroy(&account_id, &cal_id) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "cannotDestroyDefault");
            assert_eq!(
                err.description.as_deref(),
                Some("default calendar cannot be destroyed")
            );
        }
        other => panic!("expected cannotDestroyDefault Set error, got {other:?}"),
    }

    match client.calendar_update(&account_id, &cal_id, json!({"name": "Forbidden Rename"})) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "forbidden");
            assert_eq!(err.description.as_deref(), Some("permission denied"));
        }
        other => panic!("expected forbidden Set error, got {other:?}"),
    }
}

#[test]
fn calendar_create_fails_with_protocol_error_when_neither_created_nor_not_created() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Calendar/set", {
                    "accountId": "acc1",
                    "oldState": "s1",
                    "newState": "s1",
                    "created": {},
                    "notCreated": {}
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let cal = Calendar::new("Test");

    match client.calendar_create(&account_id, &cal) {
        Err(Error::Protocol(msg)) => {
            assert_eq!(msg, "/set response reports neither success nor failure");
        }
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

// =========================================================================
// Tests for CalendarEvent get chunking boundaries (maxObjectsInGet 0 vs positive)
// =========================================================================

#[test]
fn event_get_when_max_objects_in_get_is_zero_does_not_infinite_loop() {
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_clone = call_count.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(0)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let current = count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if current > 3 {
            panic!("event_get infinite looped with zero max_objects chunking!");
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = json!({
            "methodResponses": [
                ["CalendarEvent/get", {
                    "accountId": "acc1",
                    "state": "event-state-1",
                    "list": [
                        {
                            "id": "E1",
                            "title": "Meeting One",
                            "start": "2026-10-17T10:00:00",
                            "duration": "PT30M"
                        },
                        {
                            "id": "E2",
                            "title": "Meeting Two",
                            "start": "2026-10-17T11:00:00",
                            "duration": "PT30M"
                        }
                    ],
                    "notFound": []
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let response = client
        .event_get(&account_id, &[Id::new("E1"), Id::new("E2")])
        .unwrap();
    assert_eq!(response.list.len(), 2);
    assert_eq!(response.state.as_str(), "event-state-1");
}

#[test]
fn event_get_splits_across_positive_max_objects_in_get() {
    let chunk_sizes = Arc::new(Mutex::new(Vec::new()));
    let sizes_clone = chunk_sizes.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(2)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let ids = body_val["methodCalls"][0][1]["ids"].as_array().unwrap();
        sizes_clone.lock().unwrap().push(ids.len());

        let list: Vec<Value> = ids
            .iter()
            .map(|id| {
                json!({
                    "id": id,
                    "title": format!("Event {}", id.as_str().unwrap()),
                    "start": "2026-10-17T10:00:00",
                    "duration": "PT30M"
                })
            })
            .collect();

        let resp_json = json!({
            "methodResponses": [
                ["CalendarEvent/get", {
                    "accountId": "acc1",
                    "state": "event-state-2",
                    "list": list,
                    "notFound": []
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let resp = client
        .event_get(&account_id, &[Id::new("E1"), Id::new("E2"), Id::new("E3")])
        .unwrap();

    assert_eq!(resp.list.len(), 3);
    let sizes = chunk_sizes.lock().unwrap().clone();
    assert_eq!(
        sizes,
        vec![2, 1],
        "3 IDs with maxObjectsInGet=2 must split into chunks of 2 and 1"
    );
}

// =========================================================================
// Tests for CalendarEvent mutation error handling and parse demoting
// =========================================================================

#[test]
fn event_create_success() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["CalendarEvent/set", {
                    "accountId": "acc1",
                    "oldState": "s1",
                    "newState": "s2",
                    "created": {
                        "new": {
                            "id": "E-created-1",
                            "title": "Design Discussion",
                            "start": "2026-10-17T14:00:00",
                            "duration": "PT45M"
                        }
                    }
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let event = CalendarEvent::simple("CAL1", "Design Discussion", "2026-10-17T14:00:00", "PT45M");
    let created = client.event_create(&account_id, &event).unwrap();
    assert_eq!(created.id.as_ref(), Some(&Id::new("E-created-1")));
    assert_eq!(created.title.as_deref(), Some("Design Discussion"));
    assert_eq!(created.duration.as_deref(), Some("PT45M"));
}

#[test]
fn event_query_filters_and_returns_ids() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["CalendarEvent/query", {
                    "accountId": "acc1",
                    "queryState": "qs-1",
                    "canCalculateChanges": false,
                    "position": 0,
                    "total": 2,
                    "ids": ["E1", "E2"]
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let q = client
        .event_query(
            &account_id,
            jmap_proto::calendars::CalendarEventQueryFilter::in_calendar("CAL1"),
        )
        .unwrap();
    assert_eq!(q.ids, vec![Id::new("E1"), Id::new("E2")]);
    assert_eq!(q.total, Some(2));
}

#[test]
fn event_destroy_and_update_fail_when_server_reports_not_destroyed_or_not_updated() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let args = &body_val["methodCalls"][0][1];

        let resp = if args.get("destroy").is_some() {
            json!({
                "methodResponses": [
                    ["CalendarEvent/set", {
                        "accountId": "acc1",
                        "oldState": "s1",
                        "newState": "s1",
                        "notDestroyed": {
                            "E1": {
                                "type": "notFound",
                                "description": "event not found"
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["CalendarEvent/set", {
                        "accountId": "acc1",
                        "oldState": "s1",
                        "newState": "s1",
                        "notUpdated": {
                            "E1": {
                                "type": "stateMismatch",
                                "description": "ifInState mismatch"
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let event_id = Id::new("E1");

    match client.event_destroy(&account_id, &event_id) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
            assert_eq!(err.description.as_deref(), Some("event not found"));
        }
        other => panic!("expected notFound Set error, got {other:?}"),
    }

    match client.event_update(&account_id, &event_id, json!({"title": "New Title"})) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "stateMismatch");
            assert_eq!(err.description.as_deref(), Some("ifInState mismatch"));
        }
        other => panic!("expected stateMismatch Set error, got {other:?}"),
    }
}

#[test]
fn event_create_fails_with_protocol_error_when_neither_created_nor_not_created() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["CalendarEvent/set", {
                    "accountId": "acc1",
                    "oldState": "s1",
                    "newState": "s1",
                    "created": {},
                    "notCreated": {}
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let event = CalendarEvent::simple("CAL1", "Title", "2026-10-17T10:00:00", "PT30M");

    match client.event_create(&account_id, &event) {
        Err(Error::Protocol(msg)) => {
            assert_eq!(msg, "/set response reports neither success nor failure");
        }
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

#[test]
fn event_parse_handles_single_array_unwrapping_and_multi_element_demoting() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        // Server returns single array for blob1 (should unwrap),
        // and multi-element array for blob2 (should demote to notParsable).
        let resp = json!({
            "methodResponses": [
                ["CalendarEvent/parse", {
                    "accountId": "acc1",
                    "parsed": {
                        "blob1": [
                            {
                                "title": "Single Event",
                                "start": "2026-10-17T12:00:00",
                                "duration": "PT1H"
                            }
                        ],
                        "blob2": [
                            {
                                "title": "First Event",
                                "start": "2026-10-17T13:00:00",
                                "duration": "PT1H"
                            },
                            {
                                "title": "Second Event",
                                "start": "2026-10-17T14:00:00",
                                "duration": "PT1H"
                            }
                        ]
                    },
                    "notParsable": ["blob3"],
                    "notFound": ["blob4"]
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let parse_req =
        CalendarEventParseRequest::new("acc1", vec!["blob1", "blob2", "blob3", "blob4"]);
    let parsed_res = client.event_parse(&parse_req).unwrap();

    let parsed_map = parsed_res.parsed.expect("parsed map must exist");
    assert!(parsed_map.contains_key(&Id::new("blob1")));
    assert_eq!(
        parsed_map.get(&Id::new("blob1")).unwrap().title.as_deref(),
        Some("Single Event")
    );
    assert!(
        !parsed_map.contains_key(&Id::new("blob2")),
        "multi-element blob must be removed from parsed"
    );

    let not_parsable = parsed_res.not_parsable.expect("notParsable must exist");
    assert!(
        not_parsable.contains(&Id::new("blob2")),
        "multi-element blob must be demoted to notParsable"
    );
    assert!(not_parsable.contains(&Id::new("blob3")));

    let not_found = parsed_res.not_found.expect("notFound must exist");
    assert_eq!(not_found, vec![Id::new("blob4")]);
}

// =========================================================================
// Tests for CalendarEventNotification get, query, and destroy
// =========================================================================

#[test]
fn calendar_event_notifications_lists_visible_notifications() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["CalendarEventNotification/get", {
                    "accountId": "acc1",
                    "state": "notif-state-1",
                    "list": [
                        {
                            "id": "N1",
                            "created": "2026-10-17T09:00:00Z",
                            "eventId": "E100",
                            "comment": "Alice rescheduled team meeting"
                        },
                        {
                            "id": "N2",
                            "created": "2026-10-17T09:30:00Z",
                            "eventId": "E101",
                            "comment": "Bob accepted invitation"
                        }
                    ]
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let notifs = client.calendar_event_notifications(&account_id).unwrap();
    assert_eq!(notifs.len(), 2);
    assert_eq!(notifs[0].id.as_ref(), Some(&Id::new("N1")));
    assert_eq!(notifs[0].event_id, Id::new("E100"));
    assert_eq!(
        notifs[0].comment.as_deref(),
        Some("Alice rescheduled team meeting")
    );
    assert_eq!(notifs[1].id.as_ref(), Some(&Id::new("N2")));
    assert_eq!(notifs[1].event_id, Id::new("E101"));
}

#[test]
fn calendar_event_notification_query_filters_and_returns_ids() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let filter = &body_val["methodCalls"][0][1]["filter"];

        assert_eq!(
            filter["after"].as_str(),
            Some("2026-10-17T00:00:00Z"),
            "query filter after timestamp must match"
        );

        let resp = json!({
            "methodResponses": [
                ["CalendarEventNotification/query", {
                    "accountId": "acc1",
                    "queryState": "qs-1",
                    "canCalculateChanges": false,
                    "position": 0,
                    "ids": ["N1", "N2"]
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let ids = client
        .calendar_event_notification_query(
            &account_id,
            CalendarEventNotificationQueryFilter::new().with_after("2026-10-17T00:00:00Z"),
        )
        .unwrap();

    assert_eq!(ids, vec![Id::new("N1"), Id::new("N2")]);
}

#[test]
fn calendar_event_notification_destroy_success_and_error_handling() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let destroy_id = body_val["methodCalls"][0][1]["destroy"][0]
            .as_str()
            .unwrap();

        let resp = match destroy_id {
            "N-ok" => json!({
                "methodResponses": [
                    ["CalendarEventNotification/set", {
                        "accountId": "acc1",
                        "oldState": "s1",
                        "newState": "s2",
                        "destroyed": ["N-ok"]
                    }, tag]
                ],
                "sessionState": "session-state-1"
            }),
            "N-err" => json!({
                "methodResponses": [
                    ["CalendarEventNotification/set", {
                        "accountId": "acc1",
                        "oldState": "s1",
                        "newState": "s1",
                        "notDestroyed": {
                            "N-err": {
                                "type": "forbidden",
                                "description": "cannot dismiss notification"
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            }),
            "N-dropped" => json!({
                "methodResponses": [
                    ["CalendarEventNotification/set", {
                        "accountId": "acc1",
                        "oldState": "s1",
                        "newState": "s1",
                        "destroyed": [],
                        "notDestroyed": {}
                    }, tag]
                ],
                "sessionState": "session-state-1"
            }),
            other => panic!("unexpected destroy id {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");

    // Success path
    assert!(
        client
            .calendar_event_notification_destroy(&account_id, &Id::new("N-ok"))
            .is_ok()
    );

    // notDestroyed error path
    match client.calendar_event_notification_destroy(&account_id, &Id::new("N-err")) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "forbidden");
            assert_eq!(
                err.description.as_deref(),
                Some("cannot dismiss notification")
            );
        }
        other => panic!("expected forbidden Set error, got {other:?}"),
    }

    // Dropped ID fallback to notFound (Finding 20 tolerance)
    match client.calendar_event_notification_destroy(&account_id, &Id::new("N-dropped")) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
        }
        other => panic!("expected notFound Set error on dropped ID, got {other:?}"),
    }
}

// =========================================================================
// Tests for ParticipantIdentity get, create, update, destroy, and set_default
// =========================================================================

#[test]
fn participant_identities_lists_identities() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["ParticipantIdentity/get", {
                    "accountId": "acc1",
                    "state": "pi-state-1",
                    "list": [
                        {
                            "id": "PI1",
                            "name": "Alice Work",
                            "calendarAddress": "mailto:alice.work@example.com",
                            "isDefault": true
                        },
                        {
                            "id": "PI2",
                            "name": "Alice Personal",
                            "calendarAddress": "mailto:alice.home@example.com",
                            "isDefault": false
                        }
                    ]
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let identities = client.participant_identities(&account_id).unwrap();
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[0].id.as_ref(), Some(&Id::new("PI1")));
    assert_eq!(identities[0].name, "Alice Work");
    assert_eq!(
        identities[0].calendar_address.as_deref(),
        Some("mailto:alice.work@example.com")
    );
    assert_eq!(identities[0].is_default, Some(true));
    assert_eq!(identities[1].id.as_ref(), Some(&Id::new("PI2")));
    assert_eq!(identities[1].is_default, Some(false));
}

#[test]
fn participant_identity_create_success_and_error_handling() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let create_name = body_val["methodCalls"][0][1]["create"]["new"]["name"]
            .as_str()
            .unwrap();

        let resp = if create_name == "Bob Valid" {
            json!({
                "methodResponses": [
                    ["ParticipantIdentity/set", {
                        "accountId": "acc1",
                        "oldState": "pi-s1",
                        "newState": "pi-s2",
                        "created": {
                            "new": {
                                "id": "PI-new-123",
                                "name": "Bob Valid",
                                "calendarAddress": "mailto:bob@example.com",
                                "isDefault": false
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["ParticipantIdentity/set", {
                        "accountId": "acc1",
                        "oldState": "pi-s1",
                        "newState": "pi-s1",
                        "notCreated": {
                            "new": {
                                "type": "invalidProperties",
                                "description": "name must not be empty"
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");

    // Success path
    let identity =
        ParticipantIdentity::new("Bob Valid").with_calendar_address("mailto:bob@example.com");
    let created = client
        .participant_identity_create(&account_id, &identity)
        .unwrap();
    assert_eq!(created.id.as_ref(), Some(&Id::new("PI-new-123")));
    assert_eq!(created.name, "Bob Valid");
    assert_eq!(
        created.calendar_address.as_deref(),
        Some("mailto:bob@example.com")
    );

    // Error path
    let invalid = ParticipantIdentity::new("");
    match client.participant_identity_create(&account_id, &invalid) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "invalidProperties");
            assert_eq!(err.description.as_deref(), Some("name must not be empty"));
        }
        other => panic!("expected invalidProperties Set error, got {other:?}"),
    }
}

#[test]
fn participant_identity_update_and_destroy_success_and_error_handling() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let args = &body_val["methodCalls"][0][1];

        let resp = if let Some(update_map) = args.get("update") {
            if update_map.get("PI-ok").is_some() {
                json!({
                    "methodResponses": [
                        ["ParticipantIdentity/set", {
                            "accountId": "acc1",
                            "oldState": "s1",
                            "newState": "s2",
                            "updated": {"PI-ok": null}
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            } else {
                json!({
                    "methodResponses": [
                        ["ParticipantIdentity/set", {
                            "accountId": "acc1",
                            "oldState": "s1",
                            "newState": "s1",
                            "notUpdated": {
                                "PI-err": {
                                    "type": "notFound",
                                    "description": "participant identity not found"
                                }
                            }
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
        } else if let Some(destroy_list) = args.get("destroy") {
            let id = destroy_list[0].as_str().unwrap();
            if id == "PI-ok" {
                json!({
                    "methodResponses": [
                        ["ParticipantIdentity/set", {
                            "accountId": "acc1",
                            "oldState": "s1",
                            "newState": "s2",
                            "destroyed": ["PI-ok"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            } else {
                json!({
                    "methodResponses": [
                        ["ParticipantIdentity/set", {
                            "accountId": "acc1",
                            "oldState": "s1",
                            "newState": "s1",
                            "notDestroyed": {
                                "PI-default": {
                                    "type": "cannotDestroyDefault",
                                    "description": "cannot destroy default participant identity"
                                }
                            }
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
        } else {
            panic!("unexpected call: {args:?}");
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");

    // Update success
    assert!(
        client
            .participant_identity_update(
                &account_id,
                &Id::new("PI-ok"),
                json!({"name": "New Name"})
            )
            .is_ok()
    );

    // Update error
    match client.participant_identity_update(
        &account_id,
        &Id::new("PI-err"),
        json!({"name": "New Name"}),
    ) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "notFound");
            assert_eq!(
                err.description.as_deref(),
                Some("participant identity not found")
            );
        }
        other => panic!("expected notFound Set error, got {other:?}"),
    }

    // Destroy success
    assert!(
        client
            .participant_identity_destroy(&account_id, &Id::new("PI-ok"))
            .is_ok()
    );

    // Destroy error
    match client.participant_identity_destroy(&account_id, &Id::new("PI-default")) {
        Err(Error::Set(err)) => {
            assert_eq!(err.error_type, "cannotDestroyDefault");
            assert_eq!(
                err.description.as_deref(),
                Some("cannot destroy default participant identity")
            );
        }
        other => panic!("expected cannotDestroyDefault Set error, got {other:?}"),
    }
}

#[test]
fn participant_identity_set_default_sends_argument_and_propagates_errors() {
    let captured_arg = Arc::new(Mutex::new(None));
    let arg_clone = captured_arg.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let default_arg = body_val["methodCalls"][0][1]["onSuccessSetIsDefault"]
            .as_str()
            .map(|s| s.to_owned());
        *arg_clone.lock().unwrap() = default_arg;

        let resp = json!({
            "methodResponses": [
                ["ParticipantIdentity/set", {
                    "accountId": "acc1",
                    "oldState": "s1",
                    "newState": "s2"
                }, tag]
            ],
            "sessionState": "session-state-1"
        });

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let target_id = Id::new("PI-default-target");
    assert!(
        client
            .participant_identity_set_default(&account_id, &target_id)
            .is_ok()
    );

    assert_eq!(
        *captured_arg.lock().unwrap(),
        Some("PI-default-target".to_owned()),
        "participant_identity_set_default must submit onSuccessSetIsDefault to server"
    );

    // Verify error propagation on server failure
    let failing_client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(500)).to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }
        HttpResponse {
            status: 500,
            content_type: Some("text/plain".to_owned()),
            body: b"Internal Server Error".to_vec(),
            final_url: req.url.to_owned(),
        }
    });

    match failing_client.participant_identity_set_default(&account_id, &target_id) {
        Err(Error::Http { status, .. }) => assert_eq!(status, 500),
        other => panic!("expected Http 500 error, got {other:?}"),
    }
}
