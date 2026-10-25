// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Targeted mutation test coverage for jmap-client modules:
//! - principals.rs (RFC 9670 Principal/get, /query, availability, share notifications, username matching)
//! - contacts.rs (RFC 9610 AddressBook/set, /get, ContactCard/set, /get, /parse array unwrap/demote, limits)

use std::sync::Arc;
use std::sync::Mutex;

use jmap_client::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::{
    AddressBook, ContactCard, ContactCardParseRequest, ContactCardQueryFilter,
};
use jmap_proto::principals::{Principal, PrincipalQueryFilter, ShareNotificationQueryFilter};
use serde_json::{Value, json};

// =========================================================================
// Custom Transport for Wire-level Principals and Contacts Edge Cases
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
    username: &str,
    with_principals_cap: bool,
    current_user_principal_id: Option<&str>,
) -> Value {
    let mut principals_cap = json!({});
    if let Some(id) = current_user_principal_id {
        principals_cap["currentUserPrincipalId"] = json!(id);
    }

    let mut acct_caps = json!({
        "urn:ietf:params:jmap:core": {},
        "urn:ietf:params:jmap:contacts": {},
        "urn:ietf:params:jmap:calendars": {}
    });
    if with_principals_cap {
        acct_caps["urn:ietf:params:jmap:principals"] = principals_cap;
    }

    json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 50000000,
                "maxConcurrentUpload": 4,
                "maxSizeRequest": 10000000,
                "maxConcurrentRequests": 4,
                "maxCallsInRequest": 16,
                "maxObjectsInGet": 500,
                "maxObjectsInSet": 500,
                "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
            },
            "urn:ietf:params:jmap:contacts": {},
            "urn:ietf:params:jmap:principals": {}
        },
        "accounts": {
            "acc1": {
                "name": username,
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": acct_caps
            }
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:core": "acc1",
            "urn:ietf:params:jmap:contacts": "acc1",
            "urn:ietf:params:jmap:principals": "acc1"
        },
        "username": username,
        "apiUrl": "https://mail.example.internal/jmap/api",
        "downloadUrl": "https://mail.example.internal/jmap/download/{blobId}",
        "uploadUrl": "https://mail.example.internal/jmap/upload",
        "eventSourceUrl": "https://mail.example.internal/jmap/events",
        "state": "session-state-1"
    })
}

// =========================================================================
// Tests for src/principals.rs
// =========================================================================

#[test]
fn principal_get_single_id_found_never_triggers_fallback_even_when_id_matches_account() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice@example.com", true, Some("acc1"))
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap().to_owned();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap().to_owned();
        calls_clone.lock().unwrap().push(method.clone());

        let resp_json = match method.as_str() {
            "Principal/get" => {
                json!({
                    "methodResponses": [
                        ["Principal/get", {
                            "accountId": "acc1",
                            "state": "p-state-1",
                            "list": [
                                {
                                    "id": "acc1",
                                    "name": "Account As Principal",
                                    "email": "acct@example.com"
                                }
                            ],
                            "notFound": []
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            "Principal/query" => {
                json!({
                    "methodResponses": [
                        ["Principal/query", {
                            "accountId": "acc1",
                            "queryState": "q1",
                            "canCalculateChanges": false,
                            "position": 0,
                            "ids": ["owner-p"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let result = client
        .principal_get(&account_id, &[Id::new("acc1")])
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id.as_ref(), Some(&Id::new("acc1")));
    assert_eq!(result[0].name, "Account As Principal");

    let logged = calls.lock().unwrap().clone();
    assert_eq!(
        logged,
        vec!["Principal/get".to_owned()],
        "must not trigger fallback query when list is not empty"
    );
}

#[test]
fn principal_get_multiple_ids_with_first_not_found_does_not_trigger_fallback() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice@example.com", true, Some("acc1"))
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap().to_owned();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap().to_owned();
        calls_clone.lock().unwrap().push(method.clone());

        let resp_json = match method.as_str() {
            "Principal/get" => {
                json!({
                    "methodResponses": [
                        ["Principal/get", {
                            "accountId": "acc1",
                            "state": "p-state-1",
                            "list": [
                                {
                                    "id": "p2",
                                    "name": "Second Principal",
                                    "email": "p2@example.com"
                                }
                            ],
                            "notFound": ["acc1"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            "Principal/query" => {
                json!({
                    "methodResponses": [
                        ["Principal/query", {
                            "accountId": "acc1",
                            "queryState": "q1",
                            "canCalculateChanges": false,
                            "position": 0,
                            "ids": ["owner-p"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let result = client
        .principal_get(&account_id, &[Id::new("acc1"), Id::new("p2")])
        .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id.as_ref(), Some(&Id::new("p2")));
    assert_eq!(result[0].name, "Second Principal");

    let logged = calls.lock().unwrap().clone();
    assert_eq!(
        logged,
        vec!["Principal/get".to_owned()],
        "must not trigger fallback when ids.len() > 1"
    );
}

#[test]
fn principal_get_single_id_not_in_not_found_does_not_trigger_fallback() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice@example.com", true, Some("acc1"))
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap().to_owned();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap().to_owned();
        calls_clone.lock().unwrap().push(method.clone());

        let resp_json = match method.as_str() {
            "Principal/get" => {
                json!({
                    "methodResponses": [
                        ["Principal/get", {
                            "accountId": "acc1",
                            "state": "p-state-1",
                            "list": [],
                            "notFound": []
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let result = client
        .principal_get(&account_id, &[Id::new("acc1")])
        .unwrap();
    assert!(
        result.is_empty(),
        "when ids[0] is not in notFound list, fallback must not trigger"
    );

    let logged = calls.lock().unwrap().clone();
    assert_eq!(logged, vec!["Principal/get".to_owned()]);
}

#[test]
fn principal_get_fallback_when_requested_id_is_account_id_and_advertised_id_is_none() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice@example.com", true, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = match method {
            "Principal/get" => {
                let ids = body_val["methodCalls"][0][1]["ids"].as_array();
                if ids == Some(&vec![json!("acc1")]) {
                    json!({
                        "methodResponses": [
                            ["Principal/get", {
                                "accountId": "acc1",
                                "state": "p-state-1",
                                "list": [],
                                "notFound": ["acc1"]
                            }, tag]
                        ],
                        "sessionState": "session-state-1"
                    })
                } else if ids == Some(&vec![json!("resolved-owner-p")]) {
                    json!({
                        "methodResponses": [
                            ["Principal/get", {
                                "accountId": "acc1",
                                "state": "p-state-1",
                                "list": [
                                    {
                                        "id": "resolved-owner-p",
                                        "name": "Alice Example",
                                        "email": "alice@example.com"
                                    }
                                ],
                                "notFound": []
                            }, tag]
                        ],
                        "sessionState": "session-state-1"
                    })
                } else {
                    panic!("unexpected ids: {ids:?}");
                }
            }
            "Principal/query" => {
                json!({
                    "methodResponses": [
                        ["Principal/query", {
                            "accountId": "acc1",
                            "queryState": "q1",
                            "canCalculateChanges": false,
                            "position": 0,
                            "ids": ["resolved-owner-p"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let result = client
        .principal_get(&account_id, &[Id::new("acc1")])
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id.as_ref(), Some(&Id::new("resolved-owner-p")));
}

#[test]
fn principal_get_fallback_when_requested_id_matches_advertised_id_different_from_account_id() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice@example.com", true, Some("custom-adv-id"))
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = match method {
            "Principal/get" => {
                let ids = body_val["methodCalls"][0][1]["ids"].as_array();
                if ids == Some(&vec![json!("custom-adv-id")]) {
                    json!({
                        "methodResponses": [
                            ["Principal/get", {
                                "accountId": "acc1",
                                "state": "p-state-1",
                                "list": [],
                                "notFound": ["custom-adv-id"]
                            }, tag]
                        ],
                        "sessionState": "session-state-1"
                    })
                } else if ids == Some(&vec![json!("resolved-owner-p")]) {
                    json!({
                        "methodResponses": [
                            ["Principal/get", {
                                "accountId": "acc1",
                                "state": "p-state-1",
                                "list": [
                                    {
                                        "id": "resolved-owner-p",
                                        "name": "Alice Example",
                                        "email": "alice@example.com"
                                    }
                                ],
                                "notFound": []
                            }, tag]
                        ],
                        "sessionState": "session-state-1"
                    })
                } else {
                    panic!("unexpected ids: {ids:?}");
                }
            }
            "Principal/query" => {
                json!({
                    "methodResponses": [
                        ["Principal/query", {
                            "accountId": "acc1",
                            "queryState": "q1",
                            "canCalculateChanges": false,
                            "position": 0,
                            "ids": ["resolved-owner-p"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let result = client
        .principal_get(&account_id, &[Id::new("custom-adv-id")])
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id.as_ref(), Some(&Id::new("resolved-owner-p")));
}

#[test]
fn current_user_principal_account_lacking_principals_capability_returns_none() {
    let client = client_with_handler(|req| HttpResponse {
        status: 200,
        content_type: Some("application/json".to_owned()),
        body: default_session_json("alice@example.com", false, None)
            .to_string()
            .into_bytes(),
        final_url: req.url.to_owned(),
    });

    let account_id = Id::new("acc1");
    let principal = client.current_user_principal(&account_id).unwrap();
    assert!(
        principal.is_none(),
        "account lacking principals capability must return None"
    );
}

#[test]
fn resolve_owner_principal_empty_username_returns_none() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("", true, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = match method {
            "Principal/get" => {
                json!({
                    "methodResponses": [
                        ["Principal/get", {
                            "accountId": "acc1",
                            "state": "p-state-1",
                            "list": [
                                {
                                    "id": "p1",
                                    "name": "Alice",
                                    "email": "alice@example.com"
                                }
                            ],
                            "notFound": []
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let principal = client.current_user_principal(&account_id).unwrap();
    assert!(
        principal.is_none(),
        "empty username must return None without matching any principal"
    );
}

#[test]
fn resolve_owner_principal_username_without_at_matches_email_local_part_and_name() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice", true, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = match method {
            "Principal/get" => {
                let ids = body_val["methodCalls"][0][1]["ids"].as_array();
                if ids == Some(&vec![json!("p1")]) {
                    json!({
                        "methodResponses": [
                            ["Principal/get", {
                                "accountId": "acc1",
                                "state": "p-state-1",
                                "list": [
                                    {
                                        "id": "p1",
                                        "name": "Alice Example",
                                        "email": "alice@example.com"
                                    }
                                ],
                                "notFound": []
                            }, tag]
                        ],
                        "sessionState": "session-state-1"
                    })
                } else {
                    panic!("unexpected ids: {ids:?}");
                }
            }
            "Principal/query" => {
                json!({
                    "methodResponses": [
                        ["Principal/query", {
                            "accountId": "acc1",
                            "queryState": "q1",
                            "canCalculateChanges": false,
                            "position": 0,
                            "ids": ["p1"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let principal = client
        .current_user_principal(&account_id)
        .unwrap()
        .expect("must find principal");
    assert_eq!(principal.id.as_ref(), Some(&Id::new("p1")));
    assert_eq!(principal.email.as_deref(), Some("alice@example.com"));

    let id = client
        .current_user_principal_id(&account_id)
        .unwrap()
        .expect("must find principal id");
    assert_eq!(id, Id::new("p1"));
}

#[test]
fn resolve_owner_principal_username_without_at_matches_display_name_when_email_differs() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice_screen_name", true, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = match method {
            "Principal/get" => {
                let ids = body_val["methodCalls"][0][1]["ids"].as_array();
                if ids == Some(&vec![json!("p2")]) {
                    json!({
                        "methodResponses": [
                            ["Principal/get", {
                                "accountId": "acc1",
                                "state": "p-state-1",
                                "list": [
                                    {
                                        "id": "p2",
                                        "name": "alice_screen_name",
                                        "email": "different_local@example.com"
                                    }
                                ],
                                "notFound": []
                            }, tag]
                        ],
                        "sessionState": "session-state-1"
                    })
                } else {
                    panic!("unexpected ids: {ids:?}");
                }
            }
            "Principal/query" => {
                json!({
                    "methodResponses": [
                        ["Principal/query", {
                            "accountId": "acc1",
                            "queryState": "q1",
                            "canCalculateChanges": false,
                            "position": 0,
                            "ids": ["p2"]
                        }, tag]
                    ],
                    "sessionState": "session-state-1"
                })
            }
            other => panic!("unexpected method: {other}"),
        };

        HttpResponse {
            status: 200,
            content_type: Some("application/json".to_owned()),
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let principal = client
        .current_user_principal(&account_id)
        .unwrap()
        .expect("must find principal");
    assert_eq!(principal.id.as_ref(), Some(&Id::new("p2")));
    assert_eq!(principal.name, "alice_screen_name");
}

#[test]
fn share_notifications_and_query_return_matching_records() {
    let attendee = Id::new("P-bob");
    let server = MockServer::builder()
        .bearer_token("alice-token")
        .bearer_token_as("bob-token", attendee.clone())
        .start();
    let account_id = server.account_id();
    let me = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let me = account.seed_current_user_principal(Principal {
            name: "Alice".to_owned(),
            email: Some("alice@example.com".to_owned()),
            ..Principal::default()
        });
        account.seed_principal(Principal {
            id: Some(attendee.clone()),
            name: "Bob".to_owned(),
            email: Some("bob@example.com".to_owned()),
            ..Principal::default()
        });
        me
    };

    let alice_client =
        Client::connect(server.origin(), Credentials::bearer("alice-token")).unwrap();
    let bob_client = Client::connect(server.origin(), Credentials::bearer("bob-token")).unwrap();

    let work_book = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Team Contacts", true)
    };

    // Grant access to trigger a share notification for attendee:
    alice_client
        .address_book_update(
            &account_id,
            &work_book,
            json!({"shareWith": {attendee.as_str(): {"mayRead": true}}}),
        )
        .unwrap();

    // Query notifications as Bob:
    let ids = bob_client
        .share_notification_query(&account_id, ShareNotificationQueryFilter::default())
        .unwrap();
    assert!(
        !ids.is_empty(),
        "query should locate generated share notification"
    );

    let notifications = bob_client.share_notifications(&account_id).unwrap();
    assert_eq!(
        notifications.len(),
        1,
        "share_notifications must return recipient notification"
    );
    assert_eq!(notifications[0].id.as_ref(), Some(&ids[0]));

    // Verify principal queries and availability with show_details true and false:
    let query_ids = alice_client
        .principal_query(
            &account_id,
            PrincipalQueryFilter::email("alice@example.com"),
        )
        .unwrap();
    assert_eq!(query_ids, vec![me.clone()]);

    // List all principals:
    let all_principals = alice_client.principals(&account_id).unwrap();
    assert_eq!(
        all_principals.len(),
        2,
        "principals must list both seeded principals"
    );
    assert_eq!(all_principals[0].name, "Alice");
    assert_eq!(all_principals[0].id.as_ref(), Some(&me));

    // Seed a calendar and event to verify non-empty get_availability:
    let cal = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Personal", true)
    };
    let event =
        jmap_proto::calendars::CalendarEvent::simple(cal, "Meeting", "2026-10-01T10:00:00", "PT1H");
    alice_client.event_create(&account_id, &event).unwrap();

    let busy_summary = alice_client
        .get_availability(
            &account_id,
            &me,
            "2026-10-01T00:00:00",
            "2026-10-02T00:00:00",
            false,
        )
        .unwrap();
    assert_eq!(busy_summary.len(), 1, "meeting must report busy period");
    assert_eq!(busy_summary[0].utc_start.as_str(), "2026-10-01T10:00:00Z");
    assert_eq!(busy_summary[0].busy_status.as_str(), "confirmed");

    let busy_detailed = alice_client
        .get_availability(
            &account_id,
            &me,
            "2026-10-01T00:00:00",
            "2026-10-02T00:00:00",
            true,
        )
        .unwrap();
    assert_eq!(busy_detailed.len(), 1);
    assert!(
        busy_detailed[0].event.is_some(),
        "details must be included when show_details is true"
    );
}

// =========================================================================
// Tests for src/contacts.rs
// =========================================================================

#[test]
fn contact_and_address_book_update_and_destroy_fail_on_nonexistent_ids() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ghost = Id::new("nonexistent-book-999");
    let err = client
        .address_book_update(&account_id, &ghost, json!({"name": "Ghost"}))
        .expect_err("update non-existent book must fail");
    match err {
        Error::Set(set_error) => assert_eq!(set_error.error_type, "notFound"),
        other => panic!("expected Error::Set(notFound), got {other:?}"),
    }

    let err = client
        .address_book_destroy(&account_id, &ghost)
        .expect_err("destroy non-existent book must fail");
    match err {
        Error::Set(set_error) => assert_eq!(set_error.error_type, "notFound"),
        other => panic!("expected Error::Set(notFound), got {other:?}"),
    }

    let ghost_card = Id::new("nonexistent-card-999");
    let err = client
        .contact_update(&account_id, &ghost_card, json!({"name/full": "Ghost"}))
        .expect_err("update non-existent contact must fail");
    match err {
        Error::Set(set_error) => assert_eq!(set_error.error_type, "notFound"),
        other => panic!("expected Error::Set(notFound), got {other:?}"),
    }

    let err = client
        .contact_destroy(&account_id, &ghost_card)
        .expect_err("destroy non-existent contact must fail");
    match err {
        Error::Set(set_error) => assert_eq!(set_error.error_type, "notFound"),
        other => panic!("expected Error::Set(notFound), got {other:?}"),
    }
}

#[test]
fn contact_get_when_max_objects_in_get_is_zero_does_not_infinite_loop() {
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_clone = call_count.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            let mut session = default_session_json("alice@example.com", true, None);
            session["capabilities"]["urn:ietf:params:jmap:core"]["maxObjectsInGet"] = json!(0);
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: session.to_string().into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let current = count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if current > 3 {
            panic!("contact_get infinite looped with zero max_objects chunking!");
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = json!({
            "methodResponses": [
                ["ContactCard/get", {
                    "accountId": "acc1",
                    "state": "card-state-1",
                    "list": [
                        {
                            "id": "C1",
                            "name": {"full": "Person One"}
                        },
                        {
                            "id": "C2",
                            "name": {"full": "Person Two"}
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
        .contact_get(&account_id, &[Id::new("C1"), Id::new("C2")])
        .unwrap();
    assert_eq!(response.list.len(), 2);
    assert_eq!(response.state.as_str(), "card-state-1");
}

#[test]
fn contact_get_splits_across_positive_max_objects_in_get() {
    let chunk_sizes = Arc::new(Mutex::new(Vec::new()));
    let sizes_clone = chunk_sizes.clone();

    let client = client_with_handler(move |req| {
        if req.method == HttpMethod::Get {
            let mut session = default_session_json("alice@example.com", true, None);
            session["capabilities"]["urn:ietf:params:jmap:core"]["maxObjectsInGet"] = json!(2);
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: session.to_string().into_bytes(),
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
                    "name": {"full": format!("Card {}", id.as_str().unwrap())}
                })
            })
            .collect();

        let resp_json = json!({
            "methodResponses": [
                ["ContactCard/get", {
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
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let resp = client
        .contact_get(&account_id, &[Id::new("C1"), Id::new("C2"), Id::new("C3")])
        .unwrap();

    assert_eq!(resp.list.len(), 3);
    let sizes = chunk_sizes.lock().unwrap().clone();
    assert_eq!(
        sizes,
        vec![2, 1],
        "3 IDs with maxObjectsInGet=2 must split into chunks of 2 and 1"
    );
}

#[test]
fn contact_crud_and_query_against_mock_server() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let book = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Personal", true)
    };

    let books = client.address_books(&account_id).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].name, "Personal");
    assert_eq!(books[0].id.as_ref(), Some(&book));

    let c1 = ContactCard::simple(book.clone(), "Person One", "p1@example.com");
    let created1 = client.contact_create(&account_id, &c1).unwrap();
    let id1 = created1.id.unwrap();

    let c2 = ContactCard::simple(book, "Person Two", "p2@example.com");
    let created2 = client.contact_create(&account_id, &c2).unwrap();
    let id2 = created2.id.unwrap();

    let response = client.contact_get(&account_id, &[id1, id2]).unwrap();
    assert_eq!(response.list.len(), 2);
    assert_eq!(response.state, client.contact_state(&account_id).unwrap());

    let q_res = client
        .contact_query(
            &account_id,
            ContactCardQueryFilter::default().text("Person"),
        )
        .unwrap();
    assert_eq!(q_res.ids.len(), 2);
}

#[test]
fn contact_card_parse_handles_array_wrapped_single_and_multi_element_blobs() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice@example.com", true, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = json!({
            "methodResponses": [
                ["ContactCard/parse", {
                    "accountId": "acc1",
                    "parsed": {
                        "single_blob": [
                            {
                                "@type": "Card",
                                "name": {"full": "Single Card"}
                            }
                        ],
                        "multi_blob": [
                            {
                                "@type": "Card",
                                "name": {"full": "Card Part 1"}
                            },
                            {
                                "@type": "Card",
                                "name": {"full": "Card Part 2"}
                            }
                        ],
                        "empty_blob": [],
                        "bare_blob": {
                            "@type": "Card",
                            "name": {"full": "Bare Card"}
                        }
                    }
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
    let req = ContactCardParseRequest::new(
        account_id,
        vec![
            Id::new("single_blob"),
            Id::new("multi_blob"),
            Id::new("empty_blob"),
            Id::new("bare_blob"),
        ],
    );

    let resp = client.contact_card_parse(&req).unwrap();
    let parsed = resp.parsed.expect("parsed map must be present");

    // Single-element array is unwrapped into parsed:
    assert!(parsed.contains_key(&Id::new("single_blob")));
    assert_eq!(
        parsed[&Id::new("single_blob")]
            .name
            .as_ref()
            .unwrap()
            .full
            .as_deref(),
        Some("Single Card")
    );

    // Bare object is preserved in parsed:
    assert!(parsed.contains_key(&Id::new("bare_blob")));
    assert_eq!(
        parsed[&Id::new("bare_blob")]
            .name
            .as_ref()
            .unwrap()
            .full
            .as_deref(),
        Some("Bare Card")
    );

    // Multi-element array and empty array are removed from parsed and moved to not_parsable:
    assert!(!parsed.contains_key(&Id::new("multi_blob")));
    assert!(!parsed.contains_key(&Id::new("empty_blob")));

    let not_parsable = resp.not_parsable.expect("notParsable must be populated");
    assert!(not_parsable.contains(&Id::new("multi_blob")));
    assert!(not_parsable.contains(&Id::new("empty_blob")));
}

#[test]
fn set_failure_protocol_error_when_set_response_has_neither_success_nor_error() {
    let client = client_with_handler(|req| {
        if req.method == HttpMethod::Get {
            return HttpResponse {
                status: 200,
                content_type: Some("application/json".to_owned()),
                body: default_session_json("alice@example.com", true, None)
                    .to_string()
                    .into_bytes(),
                final_url: req.url.to_owned(),
            };
        }

        let body_str = std::str::from_utf8(req.body.expect("body must be present")).unwrap();
        let body_val: Value = serde_json::from_str(body_str).unwrap();
        let method = body_val["methodCalls"][0][0].as_str().unwrap();
        let tag = body_val["methodCalls"][0][2].as_str().unwrap();

        let resp_json = json!({
            "methodResponses": [
                [method, {
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
            body: resp_json.to_string().into_bytes(),
            final_url: req.url.to_owned(),
        }
    });

    let account_id = Id::new("acc1");
    let book = AddressBook::new("Test Book");
    let err = client
        .address_book_create(&account_id, &book)
        .expect_err("must fail when server returns empty created map");
    match err {
        Error::Protocol(msg) => {
            assert!(msg.contains("reports neither success nor failure"));
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }

    let card = ContactCard::simple("b1", "Test Card", "tc@example.com");
    let err = client
        .contact_create(&account_id, &card)
        .expect_err("must fail when server returns empty created map");
    match err {
        Error::Protocol(msg) => {
            assert!(msg.contains("reports neither success nor failure"));
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}
