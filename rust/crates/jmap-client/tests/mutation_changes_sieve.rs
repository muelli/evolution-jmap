// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targeted mutation test coverage for jmap-client modules:
//! - changes.rs (RFC 8620 Section 5.2 /changes, capability mappings, pagination, loop protection)
//! - sieve.rs (RFC 9661 SieveScript/get, /set, /query, and /validate)

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{ChangeSet, Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::blob::{BlobUploadRequest, UploadBlob};
use jmap_proto::sieve::{SieveScript, SieveScriptQueryFilter};
use jmap_proto::{Id, State};
use serde_json::json;

#[test]
fn changes_unknown_data_type_returns_protocol_error() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let state = State::new("init-state");

    let result = client.changes(&account_id, "UnknownWidget", &state);
    match result {
        Err(Error::Protocol(msg)) => {
            assert!(
                msg.contains("no known capability for data type UnknownWidget"),
                "unexpected protocol error message: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}

type RecordedCalls = std::sync::Arc<std::sync::Mutex<Vec<(Vec<String>, String)>>>;

struct CapabilityCheckTransport {
    recorded: RecordedCalls,
}

impl Transport for CapabilityCheckTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        if req.method == HttpMethod::Get {
            let session_body = json!({
                "capabilities": {
                    "urn:ietf:params:jmap:core": {},
                    "urn:ietf:params:jmap:mail": {},
                    "urn:ietf:params:jmap:contacts": {},
                    "urn:ietf:params:jmap:calendars": {},
                    "urn:ietf:params:jmap:submission": {}
                },
                "accounts": {
                    "acc1": {
                        "name": "user@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {
                            "urn:ietf:params:jmap:core": {},
                            "urn:ietf:params:jmap:mail": {},
                            "urn:ietf:params:jmap:contacts": {},
                            "urn:ietf:params:jmap:calendars": {},
                            "urn:ietf:params:jmap:submission": {}
                        }
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:mail": "acc1",
                    "urn:ietf:params:jmap:contacts": "acc1",
                    "urn:ietf:params:jmap:calendars": "acc1",
                    "urn:ietf:params:jmap:submission": "acc1"
                },
                "username": "user@example.com",
                "apiUrl": "https://example.com/api",
                "downloadUrl": "https://example.com/download/{blobId}",
                "uploadUrl": "https://example.com/upload",
                "eventSourceUrl": "https://example.com/events",
                "state": "init-session"
            });
            return Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".to_string()),
                body: serde_json::to_vec(&session_body).unwrap(),
                final_url: req.url.to_string(),
            });
        }

        let json: serde_json::Value = serde_json::from_slice(req.body.unwrap_or_default()).unwrap();
        let using: Vec<String> = json["using"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        let method_call = &json["methodCalls"][0];
        let method = method_call[0].as_str().unwrap_or("").to_string();
        let tag = method_call[2].as_str().unwrap_or("c0");

        self.recorded.lock().unwrap().push((using, method.clone()));

        let api_response = json!({
            "methodResponses": [
                [
                    method,
                    {
                        "accountId": "acc1",
                        "oldState": "s0",
                        "newState": "s1",
                        "hasMoreChanges": false,
                        "created": [],
                        "updated": [],
                        "destroyed": []
                    },
                    tag
                ]
            ],
            "sessionState": "init-session"
        });

        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            body: serde_json::to_vec(&api_response).unwrap(),
            final_url: req.url.to_string(),
        })
    }
}

#[test]
fn changes_data_types_map_to_expected_capabilities() {
    use jmap_proto::session::{
        CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
        CAPABILITY_SUBMISSION,
    };
    use std::sync::Arc;

    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let transport = CapabilityCheckTransport {
        recorded: Arc::clone(&recorded),
    };
    let client = Client::builder()
        .transport(transport)
        .connect("https://example.com", Credentials::none())
        .unwrap();

    let account_id = Id::new("acc1");
    let state = State::new("s0");

    let cases = [
        ("Mailbox", CAPABILITY_MAIL),
        ("Email", CAPABILITY_MAIL),
        ("Thread", CAPABILITY_MAIL),
        ("AddressBook", CAPABILITY_CONTACTS),
        ("ContactCard", CAPABILITY_CONTACTS),
        ("Calendar", CAPABILITY_CALENDARS),
        ("CalendarEvent", CAPABILITY_CALENDARS),
        ("ParticipantIdentity", CAPABILITY_CALENDARS),
        ("CalendarEventNotification", CAPABILITY_CALENDARS),
        ("EmailSubmission", CAPABILITY_SUBMISSION),
    ];

    for (type_name, _expected_cap) in cases {
        let resp = client.changes(&account_id, type_name, &state).unwrap();
        assert_eq!(resp.new_state, State::new("s1"));
        assert!(!resp.has_more_changes);
    }

    let calls = recorded.lock().unwrap();
    assert_eq!(calls.len(), cases.len());
    for (i, (type_name, expected_cap)) in cases.iter().enumerate() {
        let (using, method) = &calls[i];
        assert_eq!(
            using,
            &[CAPABILITY_CORE.to_string(), expected_cap.to_string()],
            "unexpected using capabilities for {type_name}"
        );
        assert_eq!(
            method,
            &format!("{type_name}/changes"),
            "unexpected method name for {type_name}"
        );
    }
}

#[test]
fn change_set_is_empty_evaluation() {
    let base_state = State::new("state-1");
    let empty_set = ChangeSet {
        new_state: base_state.clone(),
        created: BTreeSet::new(),
        updated: BTreeSet::new(),
        destroyed: BTreeSet::new(),
    };
    assert!(empty_set.is_empty());

    let with_created = ChangeSet {
        new_state: base_state.clone(),
        created: BTreeSet::from([Id::new("c1")]),
        updated: BTreeSet::new(),
        destroyed: BTreeSet::new(),
    };
    assert!(!with_created.is_empty());

    let with_updated = ChangeSet {
        new_state: base_state.clone(),
        created: BTreeSet::new(),
        updated: BTreeSet::from([Id::new("u1")]),
        destroyed: BTreeSet::new(),
    };
    assert!(!with_updated.is_empty());

    let with_destroyed = ChangeSet {
        new_state: base_state,
        created: BTreeSet::new(),
        updated: BTreeSet::new(),
        destroyed: BTreeSet::from([Id::new("d1")]),
    };
    assert!(!with_destroyed.is_empty());
}

struct StaticChangesTransport {
    has_more_changes: bool,
    new_state: String,
}

impl Transport for StaticChangesTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        if req.method == HttpMethod::Get {
            let session_body = json!({
                "capabilities": {
                    "urn:ietf:params:jmap:core": {},
                    "urn:ietf:params:jmap:contacts": {}
                },
                "accounts": {
                    "acc1": {
                        "name": "user@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {
                            "urn:ietf:params:jmap:core": {},
                            "urn:ietf:params:jmap:contacts": {}
                        }
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:contacts": "acc1"
                },
                "username": "user@example.com",
                "apiUrl": "https://example.com/api",
                "downloadUrl": "https://example.com/download/{blobId}",
                "uploadUrl": "https://example.com/upload",
                "eventSourceUrl": "https://example.com/events",
                "state": "init-session"
            });
            return Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".to_string()),
                body: serde_json::to_vec(&session_body).unwrap(),
                final_url: req.url.to_string(),
            });
        }

        let json: serde_json::Value = serde_json::from_slice(req.body.unwrap_or_default()).unwrap();
        let tag = json["methodCalls"][0][2].as_str().unwrap_or("c0");

        let api_response = json!({
            "methodResponses": [
                [
                    "ContactCard/changes",
                    {
                        "accountId": "acc1",
                        "oldState": "s0",
                        "newState": self.new_state,
                        "hasMoreChanges": self.has_more_changes,
                        "created": ["c1"],
                        "updated": [],
                        "destroyed": []
                    },
                    tag
                ]
            ],
            "sessionState": "init-session"
        });

        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            body: serde_json::to_vec(&api_response).unwrap(),
            final_url: req.url.to_string(),
        })
    }
}

#[test]
fn all_changes_fails_when_has_more_changes_without_state_advance() {
    let transport = StaticChangesTransport {
        has_more_changes: true,
        new_state: "s0".to_string(),
    };
    let client = Client::builder()
        .transport(transport)
        .connect("https://example.com", Credentials::none())
        .unwrap();

    let account_id = Id::new("acc1");
    let since = State::new("s0");

    let result = client.all_changes(&account_id, "ContactCard", &since);
    match result {
        Err(Error::Protocol(msg)) => {
            assert!(
                msg.contains(
                    "ContactCard/changes reports more changes without advancing the state"
                ),
                "unexpected protocol error: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}

struct InfinitePagesTransport {
    call_count: AtomicUsize,
}

impl Transport for InfinitePagesTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        if req.method == HttpMethod::Get {
            let session_body = json!({
                "capabilities": {
                    "urn:ietf:params:jmap:core": {},
                    "urn:ietf:params:jmap:contacts": {}
                },
                "accounts": {
                    "acc1": {
                        "name": "user@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {
                            "urn:ietf:params:jmap:core": {},
                            "urn:ietf:params:jmap:contacts": {}
                        }
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:contacts": "acc1"
                },
                "username": "user@example.com",
                "apiUrl": "https://example.com/api",
                "downloadUrl": "https://example.com/download/{blobId}",
                "uploadUrl": "https://example.com/upload",
                "eventSourceUrl": "https://example.com/events",
                "state": "init-session"
            });
            return Ok(HttpResponse {
                status: 200,
                content_type: Some("application/json".to_string()),
                body: serde_json::to_vec(&session_body).unwrap(),
                final_url: req.url.to_string(),
            });
        }

        let json: serde_json::Value = serde_json::from_slice(req.body.unwrap_or_default()).unwrap();
        let tag = json["methodCalls"][0][2].as_str().unwrap_or("c0");

        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        let api_response = json!({
            "methodResponses": [
                [
                    "ContactCard/changes",
                    {
                        "accountId": "acc1",
                        "oldState": format!("s{idx}"),
                        "newState": format!("s{}", idx + 1),
                        "hasMoreChanges": true,
                        "created": [format!("item-{idx}")],
                        "updated": [],
                        "destroyed": []
                    },
                    tag
                ]
            ],
            "sessionState": "init-session"
        });

        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            body: serde_json::to_vec(&api_response).unwrap(),
            final_url: req.url.to_string(),
        })
    }
}

#[test]
fn all_changes_exceeding_max_pages_reports_protocol_error() {
    let transport = InfinitePagesTransport {
        call_count: AtomicUsize::new(0),
    };
    let client = Client::builder()
        .transport(transport)
        .connect("https://example.com", Credentials::none())
        .unwrap();

    let account_id = Id::new("acc1");
    let since = State::new("s0");

    let result = client.all_changes(&account_id, "ContactCard", &since);
    match result {
        Err(Error::Protocol(msg)) => {
            assert!(
                msg.contains("ContactCard/changes never stopped reporting more changes"),
                "unexpected protocol error: {msg}"
            );
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}

#[test]
fn sieve_script_lifecycle_and_validation() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let scripts = client.sieve_scripts(&account_id).unwrap();
    assert!(scripts.is_empty());

    let script = SieveScript::new("office-filter", "B1");
    let created = client.sieve_script_create(&account_id, &script).unwrap();
    let script_id = created.id.clone().expect("server-assigned id");
    assert_eq!(created.name, "office-filter");
    assert!(!created.is_active);

    let query_all = client
        .sieve_script_query(&account_id, SieveScriptQueryFilter::new())
        .unwrap();
    assert_eq!(query_all, vec![script_id.clone()]);

    let query_name = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_name("office"),
        )
        .unwrap();
    assert_eq!(query_name, vec![script_id.clone()]);

    let query_no_match = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_name("vacation"),
        )
        .unwrap();
    assert!(query_no_match.is_empty());

    client
        .sieve_script_activate(&account_id, &script_id)
        .unwrap();
    let query_active = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_is_active(true),
        )
        .unwrap();
    assert_eq!(query_active, vec![script_id.clone()]);

    client.sieve_script_deactivate(&account_id).unwrap();
    let query_inactive = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_is_active(true),
        )
        .unwrap();
    assert!(query_inactive.is_empty());

    client
        .sieve_script_update(&account_id, &script_id, json!({"name": "renamed-filter"}))
        .unwrap();
    let updated_scripts = client.sieve_scripts(&account_id).unwrap();
    assert_eq!(updated_scripts.len(), 1);
    assert_eq!(updated_scripts[0].name, "renamed-filter");

    client
        .sieve_script_destroy(&account_id, &script_id)
        .unwrap();
    let after_destroy = client.sieve_scripts(&account_id).unwrap();
    assert!(after_destroy.is_empty());

    let uploaded = client
        .blob_upload(&BlobUploadRequest::new(account_id.clone()).create_blob(
            "blob-script-1",
            UploadBlob::from_text("require [\"fileinto\"];", "application/sieve"),
        ))
        .unwrap();
    let blob_id = uploaded
        .created
        .unwrap()
        .get("blob-script-1")
        .unwrap()
        .id
        .clone();
    let validate_resp = client.sieve_script_validate(&account_id, blob_id).unwrap();
    assert!(validate_resp.is_valid());
    assert!(validate_resp.error.is_none());

    let content_resp = client
        .sieve_script_validate_content(&account_id, b"# sieve script\nkeep;".to_vec())
        .unwrap();
    assert!(content_resp.is_valid());
    assert!(content_resp.error.is_none());
}
