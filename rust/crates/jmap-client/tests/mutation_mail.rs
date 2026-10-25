// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targeted mutation test coverage for jmap-client/src/mail.rs:
//! - Mailbox CRUD, backfills, and error handling
//! - VacationResponse get and update error handling
//! - Email query, get, state, and chunking boundaries (maxObjectsInGet zero vs positive)
//! - Email query_then_get chained vs separate and in_query_order restoration
//! - Email update, destroy, import, and parse
//! - Thread get, SearchSnippet get, and Identity get
//! - Email send chained vs separate and submission backfill
//! - EmailSubmission submit, submit_at, cancel, get, and query
//! - Upload blob size boundaries (maxSizeUpload limits and omission)
//! - Download blob same-origin vs cross-origin redirect and URL templates

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use jmap_proto::Id;
use jmap_proto::mail::{
    Email, EmailImport, EmailParseRequest, EmailQueryFilter, EmailSubmissionQueryFilter, Envelope,
    EnvelopeAddress, Mailbox, Schedule,
};
use jmap_proto::methods::Comparator;
use serde_json::{Value, json};

// =========================================================================
// Custom Transport for Scripted Mail Wire Responses
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

fn default_session_json(
    max_objects_in_get: Option<usize>,
    max_calls_in_request: Option<u64>,
    max_size_upload: Option<u64>,
) -> Value {
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
    if let Some(calls) = max_calls_in_request {
        core_cap["maxCallsInRequest"] = json!(calls);
    }
    if let Some(upload_limit) = max_size_upload {
        core_cap["maxSizeUpload"] = json!(upload_limit);
    }

    json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": core_cap,
            "urn:ietf:params:jmap:mail": {},
            "urn:ietf:params:jmap:submission": {},
            "urn:ietf:params:jmap:vacationresponse": {}
        },
        "accounts": {
            "acc1": {
                "name": "user@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:core": {},
                    "urn:ietf:params:jmap:mail": {},
                    "urn:ietf:params:jmap:submission": {},
                    "urn:ietf:params:jmap:vacationresponse": {}
                }
            }
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:core": "acc1",
            "urn:ietf:params:jmap:mail": "acc1",
            "urn:ietf:params:jmap:submission": "acc1",
            "urn:ietf:params:jmap:vacationresponse": "acc1"
        },
        "username": "user@example.com",
        "apiUrl": "https://mail.example.internal/jmap/api",
        "downloadUrl": "https://mail.example.internal/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "https://mail.example.internal/jmap/upload/{accountId}",
        "eventSourceUrl": "https://mail.example.internal/jmap/events",
        "state": "session-state-1"
    })
}

// =========================================================================
// Tests for Mailbox CRUD and error paths
// =========================================================================

#[test]
fn mailbox_get_returns_mailbox_list_and_state() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Mailbox/get", {
                    "accountId": "acc1",
                    "state": "mb-state-1",
                    "list": [
                        {"id": "mb-inbox", "name": "Inbox", "role": "inbox"},
                        {"id": "mb-sent", "name": "Sent", "role": "sent"}
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
    let resp = client.mailbox_get(&account_id).unwrap();
    assert_eq!(resp.state.as_str(), "mb-state-1");
    assert_eq!(resp.list.len(), 2);
    assert_eq!(
        resp.list[0].id.as_ref().map(|id| id.as_str()),
        Some("mb-inbox")
    );
    assert_eq!(resp.list[0].name, "Inbox");
    assert_eq!(
        resp.list[1].id.as_ref().map(|id| id.as_str()),
        Some("mb-sent")
    );
}

#[test]
fn mailbox_create_and_backfills_omitted_properties() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Mailbox/set", {
                    "accountId": "acc1",
                    "created": {
                        "new": {
                            "id": "mb-created-1",
                            "name": ""
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
    let req_mb = Mailbox::new("  Projects  ")
        .with_parent_id(Id::new("mb-inbox"))
        .with_role("archive")
        .with_sort_order(42)
        .is_subscribed(true);
    let created = client.mailbox_create(&account_id, &req_mb).unwrap();
    assert_eq!(
        created.id.as_ref().map(|id| id.as_str()),
        Some("mb-created-1")
    );
    assert_eq!(created.name, "Projects");
    assert_eq!(
        created.parent_id.as_ref().map(|id| id.as_str()),
        Some("mb-inbox")
    );
    assert_eq!(created.role.as_deref(), Some("archive"));
    assert_eq!(created.sort_order, Some(42));
    assert_eq!(created.is_subscribed, Some(true));
}

#[test]
fn mailbox_create_error_paths() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Mailbox/set", {
                    "accountId": "acc1",
                    "notCreated": {
                        "new": {
                            "type": "invalidProperties",
                            "description": "name already exists"
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
    let req_mb = Mailbox::new("Duplicate");
    match client.mailbox_create(&account_id, &req_mb) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "invalidProperties"),
        other => panic!("expected Error::Set, got {:?}", other),
    }
}

#[test]
fn mailbox_update_success_and_error_paths() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let patch = &body_val["methodCalls"][0][1]["update"];

        let resp = if patch.get("mb-fail").is_some() {
            json!({
                "methodResponses": [
                    ["Mailbox/set", {
                        "accountId": "acc1",
                        "notUpdated": {
                            "mb-fail": {"type": "notFound"}
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["Mailbox/set", {
                        "accountId": "acc1",
                        "updated": {
                            "mb-ok": {}
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
    let ok_id = Id::new("mb-ok");
    assert!(
        client
            .mailbox_update(&account_id, &ok_id, json!({"name": "Renamed"}))
            .is_ok()
    );

    let fail_id = Id::new("mb-fail");
    match client.mailbox_update(&account_id, &fail_id, json!({"name": "Fails"})) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "notFound"),
        other => panic!("expected Error::Set, got {:?}", other),
    }
}

#[test]
fn mailbox_destroy_success_and_error_paths() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let destroy = &body_val["methodCalls"][0][1]["destroy"];

        let resp = if destroy[0].as_str() == Some("mb-has-email") {
            json!({
                "methodResponses": [
                    ["Mailbox/set", {
                        "accountId": "acc1",
                        "notDestroyed": {
                            "mb-has-email": {"type": "mailboxHasEmail"}
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["Mailbox/set", {
                        "accountId": "acc1",
                        "destroyed": ["mb-empty"]
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
    let empty_id = Id::new("mb-empty");
    assert!(client.mailbox_destroy(&account_id, &empty_id).is_ok());

    let busy_id = Id::new("mb-has-email");
    match client.mailbox_destroy(&account_id, &busy_id) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "mailboxHasEmail"),
        other => panic!("expected Error::Set, got {:?}", other),
    }
}

// =========================================================================
// Tests for VacationResponse get and update
// =========================================================================

#[test]
fn vacation_response_get_success_and_empty_list_protocol_error() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let account_id = body_val["methodCalls"][0][1]["accountId"].as_str().unwrap();

        let resp = if account_id == "acc-empty" {
            json!({
                "methodResponses": [
                    ["VacationResponse/get", {
                        "accountId": "acc-empty",
                        "state": "vac-0",
                        "list": [],
                        "notFound": []
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["VacationResponse/get", {
                        "accountId": "acc1",
                        "state": "vac-1",
                        "list": [
                            {
                                "id": "singleton",
                                "isEnabled": true,
                                "textBody": "Away for holiday"
                            }
                        ],
                        "notFound": []
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
    let vac = client.vacation_response_get(&account_id).unwrap();
    assert!(vac.is_enabled);
    assert_eq!(vac.text_body.as_deref(), Some("Away for holiday"));

    let empty_acct = Id::new("acc-empty");
    match client.vacation_response_get(&empty_acct) {
        Err(Error::Protocol(msg)) => assert!(msg.contains("no singleton")),
        other => panic!("expected Error::Protocol, got {:?}", other),
    }
}

#[test]
fn vacation_response_update_success_and_error_paths() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let account_id = body_val["methodCalls"][0][1]["accountId"].as_str().unwrap();

        let resp = if account_id == "acc-fail" {
            json!({
                "methodResponses": [
                    ["VacationResponse/set", {
                        "accountId": "acc-fail",
                        "notUpdated": {
                            "singleton": {"type": "forbidden"}
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["VacationResponse/set", {
                        "accountId": "acc1",
                        "updated": {
                            "singleton": {}
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
    assert!(
        client
            .vacation_response_update(&account_id, json!({"isEnabled": false}))
            .is_ok()
    );

    let fail_acct = Id::new("acc-fail");
    match client.vacation_response_update(&fail_acct, json!({"isEnabled": true})) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "forbidden"),
        other => panic!("expected Error::Set, got {:?}", other),
    }
}

// =========================================================================
// Tests for Email query, state, get, and chunking boundaries
// =========================================================================

#[test]
fn email_query_sends_parameters_and_returns_query_response() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let args = &body_val["methodCalls"][0][1];

        assert_eq!(args["accountId"], "acc1");
        assert_eq!(args["position"], 5);
        assert_eq!(args["limit"], 25);
        assert!(args["sort"].is_array());

        let resp = json!({
            "methodResponses": [
                ["Email/query", {
                    "accountId": "acc1",
                    "queryState": "qs-1",
                    "canCalculateChanges": true,
                    "position": 5,
                    "ids": ["msg1", "msg2"],
                    "total": 42,
                    "limit": 25
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
    let filter = EmailQueryFilter::default();
    let sort = Some(vec![Comparator::ascending("receivedAt")]);
    let query_resp = client
        .email_query(&account_id, filter, sort, Some(25), 5)
        .unwrap();
    assert_eq!(query_resp.ids.len(), 2);
    assert_eq!(query_resp.ids[0].as_str(), "msg1");
    assert_eq!(query_resp.total, Some(42));
    assert_eq!(query_resp.position, 5);
    assert!(query_resp.can_calculate_changes);
}

#[test]
fn email_state_returns_state_without_fetching_messages() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let args = &body_val["methodCalls"][0][1];

        assert_eq!(args["ids"], json!([]));
        assert_eq!(args["properties"], json!(["id"]));

        let resp = json!({
            "methodResponses": [
                ["Email/get", {
                    "accountId": "acc1",
                    "state": "email-state-456",
                    "list": [],
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
    let state = client.email_state(&account_id).unwrap();
    assert_eq!(state.as_str(), "email-state-456");
}

#[test]
fn email_get_chunking_with_max_objects_in_get() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&call_count);

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(2), None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        count_clone.fetch_add(1, Ordering::SeqCst);
        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let ids = body_val["methodCalls"][0][1]["ids"].as_array().unwrap();

        let list: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": id.as_str().unwrap(), "subject": "Test"}))
            .collect();

        let resp = json!({
            "methodResponses": [
                ["Email/get", {
                    "accountId": "acc1",
                    "state": "s1",
                    "list": list,
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
    let ids = vec![Id::new("m1"), Id::new("m2"), Id::new("m3")];
    let emails = client.email_get(&account_id, &ids, None).unwrap();
    assert_eq!(emails.len(), 3);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
fn email_get_with_max_objects_zero_fetches_in_one_call() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&call_count);

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(Some(0), None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        count_clone.fetch_add(1, Ordering::SeqCst);
        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let ids = body_val["methodCalls"][0][1]["ids"].as_array().unwrap();

        let list: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": id.as_str().unwrap(), "subject": "Test"}))
            .collect();

        let resp = json!({
            "methodResponses": [
                ["Email/get", {
                    "accountId": "acc1",
                    "state": "s1",
                    "list": list,
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
    let ids = vec![Id::new("m1"), Id::new("m2"), Id::new("m3")];
    let emails = client.email_get(&account_id, &ids, None).unwrap();
    assert_eq!(emails.len(), 3);
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

// =========================================================================
// Tests for Email query_then_get chained vs separate
// =========================================================================

#[test]
fn email_query_then_get_chained_when_calls_allowed() {
    let req_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&req_count);

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, Some(16), None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        count_clone.fetch_add(1, Ordering::SeqCst);
        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();

        assert_eq!(body_val["methodCalls"].as_array().unwrap().len(), 2);
        let q_tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let g_tag = body_val["methodCalls"][1][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Email/query", {
                    "accountId": "acc1",
                    "queryState": "qs-1",
                    "ids": ["m2", "m1"]
                }, q_tag],
                ["Email/get", {
                    "accountId": "acc1",
                    "state": "s1",
                    "list": [
                        {"id": "m1", "subject": "First"},
                        {"id": "m2", "subject": "Second"}
                    ],
                    "notFound": []
                }, g_tag]
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
    let emails = client
        .email_query_then_get(&account_id, EmailQueryFilter::default(), None, None)
        .unwrap();

    assert_eq!(req_count.load(Ordering::SeqCst), 1);
    assert_eq!(emails.len(), 2);
    assert_eq!(emails[0].id.as_ref().map(|id| id.as_str()), Some("m2"));
    assert_eq!(emails[1].id.as_ref().map(|id| id.as_str()), Some("m1"));
}

#[test]
fn email_query_then_get_separate_when_max_calls_is_one() {
    let req_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&req_count);

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, Some(1), None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let idx = count_clone.fetch_add(1, Ordering::SeqCst);
        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = if idx == 0 {
            json!({
                "methodResponses": [
                    ["Email/query", {
                        "accountId": "acc1",
                        "queryState": "qs-1",
                        "ids": ["m2", "m1"]
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["Email/get", {
                        "accountId": "acc1",
                        "state": "s1",
                        "list": [
                            {"id": "m1", "subject": "First"},
                            {"id": "m2", "subject": "Second"}
                        ],
                        "notFound": []
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
    let emails = client
        .email_query_then_get(&account_id, EmailQueryFilter::default(), None, None)
        .unwrap();

    assert_eq!(req_count.load(Ordering::SeqCst), 2);
    assert_eq!(emails.len(), 2);
    assert_eq!(emails[0].id.as_ref().map(|id| id.as_str()), Some("m2"));
    assert_eq!(emails[1].id.as_ref().map(|id| id.as_str()), Some("m1"));
}

#[test]
fn email_query_then_get_separate_empty_query_returns_immediately() {
    let req_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&req_count);

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, Some(1), None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        count_clone.fetch_add(1, Ordering::SeqCst);
        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Email/query", {
                    "accountId": "acc1",
                    "queryState": "qs-1",
                    "ids": []
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
    let emails = client
        .email_query_then_get(&account_id, EmailQueryFilter::default(), None, None)
        .unwrap();

    assert_eq!(req_count.load(Ordering::SeqCst), 1);
    assert!(emails.is_empty());
}

// =========================================================================
// Tests for Email update, destroy, import, parse
// =========================================================================

#[test]
fn email_update_success_refusal_and_protocol_error() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let update = &body_val["methodCalls"][0][1]["update"];

        let resp = if update.get("m-fail").is_some() {
            json!({
                "methodResponses": [
                    ["Email/set", {
                        "accountId": "acc1",
                        "notUpdated": {
                            "m-fail": {"type": "notFound"}
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else if update.get("m-ambiguous").is_some() {
            json!({
                "methodResponses": [
                    ["Email/set", {
                        "accountId": "acc1",
                        "updated": {},
                        "notUpdated": {}
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["Email/set", {
                        "accountId": "acc1",
                        "updated": {
                            "m-ok": {}
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
    assert!(
        client
            .email_update(
                &account_id,
                &Id::new("m-ok"),
                json!({"/keywords/$seen": true})
            )
            .is_ok()
    );

    match client.email_update(
        &account_id,
        &Id::new("m-fail"),
        json!({"/keywords/$seen": true}),
    ) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "notFound"),
        other => panic!("expected Error::Set, got {:?}", other),
    }

    match client.email_update(
        &account_id,
        &Id::new("m-ambiguous"),
        json!({"/keywords/$seen": true}),
    ) {
        Err(Error::Protocol(msg)) => assert!(msg.contains("m-ambiguous")),
        other => panic!("expected Error::Protocol, got {:?}", other),
    }
}

#[test]
fn email_destroy_success_refusal_and_protocol_error() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let destroy = &body_val["methodCalls"][0][1]["destroy"];

        let resp = if destroy[0].as_str() == Some("m-fail") {
            json!({
                "methodResponses": [
                    ["Email/set", {
                        "accountId": "acc1",
                        "notDestroyed": {
                            "m-fail": {"type": "notFound"}
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else if destroy[0].as_str() == Some("m-ambiguous") {
            json!({
                "methodResponses": [
                    ["Email/set", {
                        "accountId": "acc1",
                        "destroyed": [],
                        "notDestroyed": {}
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["Email/set", {
                        "accountId": "acc1",
                        "destroyed": ["m-ok"]
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
    assert!(client.email_destroy(&account_id, &Id::new("m-ok")).is_ok());

    match client.email_destroy(&account_id, &Id::new("m-fail")) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "notFound"),
        other => panic!("expected Error::Set, got {:?}", other),
    }

    match client.email_destroy(&account_id, &Id::new("m-ambiguous")) {
        Err(Error::Protocol(msg)) => assert!(msg.contains("m-ambiguous")),
        other => panic!("expected Error::Protocol, got {:?}", other),
    }
}

#[test]
fn email_import_success_refusal_and_protocol_error() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let blob_id = body_val["methodCalls"][0][1]["emails"]["imported"]["blobId"]
            .as_str()
            .unwrap();

        let resp = if blob_id == "blob-fail" {
            json!({
                "methodResponses": [
                    ["Email/import", {
                        "accountId": "acc1",
                        "newState": "import-s1",
                        "notCreated": {
                            "imported": {"type": "invalidEmail"}
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else if blob_id == "blob-ambiguous" {
            json!({
                "methodResponses": [
                    ["Email/import", {
                        "accountId": "acc1",
                        "newState": "import-s1",
                        "created": {},
                        "notCreated": {}
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["Email/import", {
                        "accountId": "acc1",
                        "newState": "import-s1",
                        "created": {
                            "imported": {
                                "id": "email-imported-1",
                                "blobId": blob_id,
                                "size": 1024
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
    let import_ok = EmailImport::new(Id::new("blob-ok"), Id::new("mb-inbox"));
    let imported = client.email_import(&account_id, &import_ok).unwrap();
    assert_eq!(
        imported.id.as_ref().map(|id| id.as_str()),
        Some("email-imported-1")
    );

    let import_fail = EmailImport::new(Id::new("blob-fail"), Id::new("mb-inbox"));
    match client.email_import(&account_id, &import_fail) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "invalidEmail"),
        other => panic!("expected Error::Set, got {:?}", other),
    }

    let import_amb = EmailImport::new(Id::new("blob-ambiguous"), Id::new("mb-inbox"));
    match client.email_import(&account_id, &import_amb) {
        Err(Error::Protocol(msg)) => assert!(msg.contains("imported")),
        other => panic!("expected Error::Protocol, got {:?}", other),
    }
}

#[test]
fn email_parse_returns_response() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Email/parse", {
                    "accountId": "acc1",
                    "parsed": {
                        "b1": {"id": "p1", "subject": "Parsed Subject"}
                    },
                    "notParsable": ["b2"],
                    "notFound": ["b3"]
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

    let req = EmailParseRequest::new(
        Id::new("acc1"),
        [Id::new("b1"), Id::new("b2"), Id::new("b3")],
    );
    let res = client.email_parse(&req).unwrap();
    assert!(res.parsed.is_some_and(|p| p.contains_key(&Id::new("b1"))));
    assert!(
        res.not_parsable
            .is_some_and(|np| np.contains(&Id::new("b2")))
    );
}

// =========================================================================
// Tests for Thread, SearchSnippet, and Identity
// =========================================================================

#[test]
fn thread_get_returns_threads() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Thread/get", {
                    "accountId": "acc1",
                    "state": "th-state-1",
                    "list": [
                        {"id": "th1", "emailIds": ["m1", "m2"]}
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
    let threads = client.thread_get(&account_id, [Id::new("th1")]).unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id.as_ref().map(|id| id.as_str()), Some("th1"));
    assert_eq!(threads[0].email_ids.len(), 2);
}

#[test]
fn search_snippet_get_with_and_without_filter() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["SearchSnippet/get", {
                    "accountId": "acc1",
                    "list": [
                        {
                            "emailId": "m1",
                            "subject": "<mark>Invoice</mark>",
                            "preview": "Your <mark>invoice</mark> is ready"
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
    let snippets = client
        .search_snippet_get(
            &account_id,
            [Id::new("m1")],
            Some(EmailQueryFilter::default()),
        )
        .unwrap();
    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].email_id.as_str(), "m1");
    assert_eq!(snippets[0].subject.as_deref(), Some("<mark>Invoice</mark>"));

    let snippets_no_filter = client
        .search_snippet_get(
            &account_id,
            [Id::new("m1")],
            Option::<EmailQueryFilter>::None,
        )
        .unwrap();
    assert_eq!(snippets_no_filter.len(), 1);
}

#[test]
fn identities_returns_identity_list() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Identity/get", {
                    "accountId": "acc1",
                    "state": "ident-state-1",
                    "list": [
                        {
                            "id": "ident-1",
                            "name": "Alice User",
                            "email": "alice@example.com",
                            "maySend": true
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
    let ids = client.identities(&account_id).unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].id.as_ref().map(|id| id.as_str()), Some("ident-1"));
    assert_eq!(ids[0].email, "alice@example.com");
    assert_eq!(ids[0].may_send, Some(true));
}

// =========================================================================
// Tests for Email send chained vs separate
// =========================================================================

#[test]
fn send_email_chained_when_calls_allowed() {
    let req_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&req_count);

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, Some(16), None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        count_clone.fetch_add(1, Ordering::SeqCst);
        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();

        assert_eq!(body_val["methodCalls"].as_array().unwrap().len(), 2);
        let e_tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let s_tag = body_val["methodCalls"][1][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Email/set", {
                    "accountId": "acc1",
                    "created": {
                        "draft": {
                            "id": "email-draft-created",
                            "blobId": "blob-1"
                        }
                    }
                }, e_tag],
                ["EmailSubmission/set", {
                    "accountId": "acc1",
                    "created": {
                        "submission": {
                            "id": "sub-created-1"
                        }
                    }
                }, s_tag]
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
    let identity_id = Id::new("ident1");
    let email = Email::new();
    let (created_email, sub) = client
        .send_email(&account_id, &email, &identity_id, None)
        .unwrap();

    assert_eq!(req_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        created_email.id.as_ref().map(|id| id.as_str()),
        Some("email-draft-created")
    );
    assert_eq!(sub.id.as_ref().map(|id| id.as_str()), Some("sub-created-1"));
    assert_eq!(sub.identity_id.as_str(), "ident1");
    assert_eq!(sub.email_id.as_str(), "email-draft-created");
}

#[test]
fn send_email_separate_when_max_calls_is_one() {
    let req_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&req_count);

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, Some(1), None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let idx = count_clone.fetch_add(1, Ordering::SeqCst);
        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = if idx == 0 {
            json!({
                "methodResponses": [
                    ["Email/set", {
                        "accountId": "acc1",
                        "created": {
                            "draft": {
                                "id": "email-draft-sep",
                                "blobId": "blob-sep"
                            }
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["EmailSubmission/set", {
                        "accountId": "acc1",
                        "created": {
                            "submission": {
                                "id": "sub-created-sep"
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
    let identity_id = Id::new("ident1");
    let email = Email::new();
    let (created_email, sub) = client
        .send_email(&account_id, &email, &identity_id, None)
        .unwrap();

    assert_eq!(req_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        created_email.id.as_ref().map(|id| id.as_str()),
        Some("email-draft-sep")
    );
    assert_eq!(
        sub.id.as_ref().map(|id| id.as_str()),
        Some("sub-created-sep")
    );
    assert_eq!(sub.email_id.as_str(), "email-draft-sep");
}

#[test]
fn send_email_separate_error_when_created_draft_has_no_id() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, Some(1), None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["Email/set", {
                    "accountId": "acc1",
                    "created": {
                        "draft": {
                            "blobId": "blob-no-id"
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
    let identity_id = Id::new("ident1");
    let email = Email::new();
    match client.send_email(&account_id, &email, &identity_id, None) {
        Err(Error::Protocol(msg)) => assert!(msg.contains("without an id")),
        other => panic!("expected Error::Protocol, got {:?}", other),
    }
}

// =========================================================================
// Tests for EmailSubmission submit, submit_at, cancel, get, and query
// =========================================================================

#[test]
fn submit_email_and_submit_email_at() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["EmailSubmission/set", {
                    "accountId": "acc1",
                    "created": {
                        "submission": {
                            "id": "sub-submitted-1"
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
    let email_id = Id::new("m1");
    let identity_id = Id::new("ident1");
    let sub = client
        .submit_email(&account_id, &email_id, &identity_id, None, None)
        .unwrap();
    assert_eq!(
        sub.id.as_ref().map(|id| id.as_str()),
        Some("sub-submitted-1")
    );
    assert_eq!(sub.email_id.as_str(), "m1");
    assert_eq!(sub.identity_id.as_str(), "ident1");

    let env = Envelope::new(
        EnvelopeAddress::new("sender@example.com"),
        [EnvelopeAddress::new("rcpt@example.com")],
    );
    let schedule = Schedule::HoldFor(300);
    let sub_at = client
        .submit_email_at(&account_id, &email_id, &identity_id, env, &schedule, None)
        .unwrap();
    assert_eq!(
        sub_at.id.as_ref().map(|id| id.as_str()),
        Some("sub-submitted-1")
    );
}

#[test]
fn cancel_email_submission_success_and_error_paths() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();
        let update = &body_val["methodCalls"][0][1]["update"];

        assert_eq!(
            update["sub1"]["undoStatus"]
                .as_str()
                .or(update["sub-fail"]["undoStatus"].as_str()),
            Some("canceled")
        );

        let resp = if update.get("sub-fail").is_some() {
            json!({
                "methodResponses": [
                    ["EmailSubmission/set", {
                        "accountId": "acc1",
                        "notUpdated": {
                            "sub-fail": {"type": "forbidden"}
                        }
                    }, tag]
                ],
                "sessionState": "session-state-1"
            })
        } else {
            json!({
                "methodResponses": [
                    ["EmailSubmission/set", {
                        "accountId": "acc1",
                        "updated": {
                            "sub1": {}
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
    assert!(
        client
            .cancel_email_submission(&account_id, &Id::new("sub1"))
            .is_ok()
    );

    match client.cancel_email_submission(&account_id, &Id::new("sub-fail")) {
        Err(Error::Set(err)) => assert_eq!(err.error_type, "forbidden"),
        other => panic!("expected Error::Set, got {:?}", other),
    }
}

#[test]
fn email_submission_get_returns_submissions() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["EmailSubmission/get", {
                    "accountId": "acc1",
                    "state": "sub-state-1",
                    "list": [
                        {
                            "id": "sub1",
                            "identityId": "ident1",
                            "emailId": "m1"
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
    let subs = client
        .email_submission_get(&account_id, [Id::new("sub1")])
        .unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].id.as_ref().map(|id| id.as_str()), Some("sub1"));
    assert_eq!(subs[0].email_id.as_str(), "m1");
}

#[test]
fn email_submission_query_returns_ids() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp = json!({
            "methodResponses": [
                ["EmailSubmission/query", {
                    "accountId": "acc1",
                    "queryState": "qs-1",
                    "ids": ["sub1", "sub2"]
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
    let filter = EmailSubmissionQueryFilter::default();
    let ids = client.email_submission_query(&account_id, filter).unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].as_str(), "sub1");
    assert_eq!(ids[1].as_str(), "sub2");
}

// =========================================================================
// Tests for Upload blob size boundaries and Download blob
// =========================================================================

#[test]
fn upload_blob_slice_and_upload_blob_size_boundaries() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json(None, None, Some(100))
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        assert_eq!(req.method, HttpMethod::Post);
        let resp = json!({
            "accountId": "acc1",
            "blobId": "uploaded-b1",
            "type": "text/plain",
            "size": req.body.as_ref().map(|b| b.len()).unwrap_or(0)
        });

        HttpResponse {
            status: 201,
            content_type: Some("application/json".to_owned()),
            body: resp.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");

    // Exact limit: 100 bytes must succeed
    let data_100 = vec![b'a'; 100];
    let res = client
        .upload_blob_slice(&account_id, "text/plain", &data_100)
        .unwrap();
    assert_eq!(res.blob_id.as_str(), "uploaded-b1");
    assert_eq!(res.size, 100);

    // Over limit: 101 bytes returns Error::TooLarge
    let data_101 = vec![b'a'; 101];
    match client.upload_blob_slice(&account_id, "text/plain", &data_101) {
        Err(Error::TooLarge { size, limit }) => {
            assert_eq!(size, 101);
            assert_eq!(limit, 100);
        }
        other => panic!("expected Error::TooLarge, got {:?}", other),
    }

    // Over limit: 102 bytes returns Error::TooLarge
    let data_102 = vec![b'a'; 102];
    match client.upload_blob_slice(&account_id, "text/plain", &data_102) {
        Err(Error::TooLarge { size, limit }) => {
            assert_eq!(size, 102);
            assert_eq!(limit, 100);
        }
        other => panic!("expected Error::TooLarge, got {:?}", other),
    }

    // upload_blob with owned vector
    let res_owned = client
        .upload_blob(&account_id, "text/plain", vec![b'x'; 50])
        .unwrap();
    assert_eq!(res_owned.blob_id.as_str(), "uploaded-b1");
    assert_eq!(res_owned.size, 50);
}

#[test]
fn download_blob_same_origin_and_cross_origin_redirect() {
    let client =
        client_with_handler(|req| {
            if req.url.contains(".well-known/jmap") {
                return HttpResponse {
                    status: 200,
                    content_type: Some("application/json".to_owned()),
                    body: default_session_json(None, None, None)
                        .to_string()
                        .into_bytes(),
                    final_url: req.url.to_owned(),
                };
            }

            assert_eq!(req.method, HttpMethod::Get);

            if req.url.contains("blob-cross") {
                return HttpResponse {
                    status: 200,
                    content_type: Some("application/octet-stream".to_owned()),
                    body: vec![1, 2, 3],
                    final_url: "https://attacker.example.com/blob-cross".to_owned(),
                };
            }

            // Verify template substitution
            assert!(req.url.contains(
                "/jmap/download/acc1/blob-ok/my%20file.txt?type=application%2Foctet-stream"
            ));

            HttpResponse {
                status: 200,
                content_type: Some("application/octet-stream".to_owned()),
                body: vec![0x10, 0x20, 0x30],
                final_url: req.url.to_owned(),
            }
        });

    let account_id = Id::new("acc1");
    let blob_ok = Id::new("blob-ok");
    let bytes = client
        .download_blob(&account_id, &blob_ok, "my file.txt", 1024)
        .unwrap();
    assert_eq!(bytes, vec![0x10, 0x20, 0x30]);

    let blob_cross = Id::new("blob-cross");
    match client.download_blob(&account_id, &blob_cross, "file.bin", 1024) {
        Err(Error::CrossOriginRedirect {
            requested,
            followed,
            ..
        }) => {
            assert!(requested.contains("mail.example.internal"));
            assert!(followed.contains("attacker.example.com"));
        }
        other => panic!("expected Error::CrossOriginRedirect, got {:?}", other),
    }
}
