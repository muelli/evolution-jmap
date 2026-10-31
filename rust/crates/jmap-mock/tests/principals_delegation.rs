// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for Principal methods, delegation, and availability in jmap-mock.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::{CalendarEvent, CalendarRights};
use jmap_proto::principals::Principal;
use serde_json::{Value, json};

struct DelegationFixture {
    server: MockServer,
    owner_principal_id: Id,
    grantee_principal_id: Id,
    shared_account_id: Id,
    fb_calendar_id: Id,
    reviewer_calendar_id: Id,
    normal_event_id: Id,
    private_event_id: Id,
    secret_event_id: Id,
}

fn api(server: &MockServer, authorization: Option<&str>, body: Value) -> Value {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");

    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let mut headers = format!(
        "POST /jmap HTTP/1.1\r\n\
         Host: {address}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n",
        payload.len()
    );
    if let Some(authorization) = authorization {
        headers.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    headers.push_str("Connection: close\r\n\r\n");

    stream.write_all(headers.as_bytes()).expect("write headers");
    stream.write_all(&payload).expect("write body");
    stream.flush().expect("flush request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the response");

    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("a response with a header block");
    assert!(head.contains("200"), "{head}");
    serde_json::from_str(body).expect("a response JSON body")
}

fn delegation_fixture(disable_principal_query: bool) -> DelegationFixture {
    let owner_principal_id = Id::from("P-owner");
    let grantee_principal_id = Id::from("P-grantee");
    let shared_account_id = Id::from("A2");

    let mut builder = MockServer::builder()
        .bearer_token_as("grantee-token", grantee_principal_id.clone())
        .bearer_token_as("owner-token", owner_principal_id.clone());

    if disable_principal_query {
        builder = builder.disable_principal_query();
    }

    let server = builder.start();

    let (fb_calendar_id, reviewer_calendar_id, normal_event_id, private_event_id, secret_event_id) = {
        let state = server.state();
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        {
            let grantee_account = state
                .account_mut(&server.account_id())
                .expect("grantee account exists");
            grantee_account.current_user_principal_id = Some(grantee_principal_id.clone());
            let mut grantee_principal = Principal::new("Grantee User");
            grantee_principal.email = Some("grantee@example.test".to_owned());
            grantee_account.principals.seed_with_id(
                grantee_principal_id.clone(),
                grantee_principal.with_id(grantee_principal_id.clone()),
            );
        }

        let shared = state.add_account(shared_account_id.clone(), "owner@example.test");
        shared.is_personal = false;
        shared.current_user_principal_id = Some(owner_principal_id.clone());

        let mut owner_principal = Principal::new("Owner User");
        owner_principal.email = Some("owner@example.test".to_owned());
        shared.principals.seed_with_id(
            owner_principal_id.clone(),
            owner_principal.with_id(owner_principal_id.clone()),
        );

        let fb_calendar_id = shared.seed_calendar("FreeBusy Only Calendar", false);
        let reviewer_calendar_id = shared.seed_calendar("Reviewer Calendar", false);

        if let Some(cal) = shared.calendars.get_mut(&fb_calendar_id) {
            let mut rights = CalendarRights::default();
            rights.may_read_free_busy = Some(true);
            rights.may_read_items = Some(false);
            cal.share_with = Some(BTreeMap::from([(grantee_principal_id.clone(), rights)]));
        }

        if let Some(cal) = shared.calendars.get_mut(&reviewer_calendar_id) {
            let mut rights = CalendarRights::default();
            rights.may_read_free_busy = Some(true);
            rights.may_read_items = Some(true);
            cal.share_with = Some(BTreeMap::from([(grantee_principal_id.clone(), rights)]));
        }

        let mut event1 = CalendarEvent::default();
        event1.title = Some("Team Meeting".to_owned());
        event1.start = Some("2026-08-01T10:00:00".to_owned());
        event1.duration = Some("PT1H".to_owned());
        event1.calendar_ids = Some(BTreeMap::from([(reviewer_calendar_id.clone(), true)]));
        event1.privacy = Some("public".to_owned());
        let normal_event_id = shared.seed_calendar_event(event1);

        let mut event2 = CalendarEvent::default();
        event2.title = Some("Confidential Doctor".to_owned());
        event2.start = Some("2026-08-01T14:00:00".to_owned());
        event2.duration = Some("PT1H".to_owned());
        event2.calendar_ids = Some(BTreeMap::from([(reviewer_calendar_id.clone(), true)]));
        event2.privacy = Some("private".to_owned());
        let private_event_id = shared.seed_calendar_event(event2);

        let mut event3 = CalendarEvent::default();
        event3.title = Some("Secret Interview".to_owned());
        event3.start = Some("2026-08-01T16:00:00".to_owned());
        event3.duration = Some("PT1H".to_owned());
        event3.calendar_ids = Some(BTreeMap::from([(reviewer_calendar_id.clone(), true)]));
        event3.privacy = Some("secret".to_owned());
        let secret_event_id = shared.seed_calendar_event(event3);

        (
            fb_calendar_id,
            reviewer_calendar_id,
            normal_event_id,
            private_event_id,
            secret_event_id,
        )
    };

    DelegationFixture {
        server,
        owner_principal_id,
        grantee_principal_id,
        shared_account_id,
        fb_calendar_id,
        reviewer_calendar_id,
        normal_event_id,
        private_event_id,
        secret_event_id,
    }
}

#[test]
fn principal_get_and_query_default_behavior() {
    let fixture = delegation_fixture(false);

    let response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:principals"
            ],
            "methodCalls": [
                ["Principal/get", {"accountId": fixture.server.account_id().as_str(), "ids": [fixture.grantee_principal_id.as_str(), fixture.owner_principal_id.as_str(), "nonexistent"]}, "p1"],
                ["Principal/query", {"accountId": fixture.server.account_id().as_str(), "filter": {"text": "Grantee"}}, "p2"]
            ]
        }),
    );

    let get_resp = &response["methodResponses"][0][1];
    let list = get_resp["list"].as_array().expect("principal list");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["name"], "Grantee User");
    assert_eq!(list[1]["name"], "Owner User");

    let not_found = get_resp["notFound"].as_array().expect("notFound list");
    assert_eq!(not_found, &[json!("nonexistent")]);

    let query_resp = &response["methodResponses"][1][1];
    let query_ids = query_resp["ids"].as_array().expect("query ids");
    assert_eq!(query_ids.len(), 1);
    assert_eq!(query_ids[0], json!(fixture.grantee_principal_id.as_str()));
}

#[test]
fn principal_query_disabled_flag_matches_stalwart_forbidden() {
    let fixture = delegation_fixture(true);

    let response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:principals"
            ],
            "methodCalls": [
                ["Principal/query", {"accountId": fixture.server.account_id().as_str(), "filter": null}, "p1"]
            ]
        }),
    );

    let method_response = &response["methodResponses"][0];
    assert_eq!(method_response[0], "error");
    assert_eq!(method_response[1]["type"], "forbidden");
    assert_eq!(
        method_response[1]["description"],
        "The administrator has disabled directory queries."
    );
}

#[test]
fn calendar_share_with_free_busy_only_hides_collection_and_events() {
    let fixture = delegation_fixture(false);

    let response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:calendars"
            ],
            "methodCalls": [
                ["Calendar/get", {"accountId": fixture.shared_account_id.as_str(), "ids": [fixture.fb_calendar_id.as_str(), fixture.reviewer_calendar_id.as_str()]}, "c1"]
            ]
        }),
    );

    let calendar_get_resp = &response["methodResponses"][0][1];
    let list = calendar_get_resp["list"].as_array().expect("calendar list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], json!(fixture.reviewer_calendar_id.as_str()));

    let not_found = calendar_get_resp["notFound"]
        .as_array()
        .expect("notFound list");
    assert!(not_found.contains(&json!(fixture.fb_calendar_id.as_str())));

    let event_response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:calendars"
            ],
            "methodCalls": [
                [
                    "CalendarEvent/get",
                    {
                        "accountId": fixture.shared_account_id.as_str(),
                        "ids": [
                            fixture.normal_event_id.as_str(),
                            fixture.private_event_id.as_str(),
                            fixture.secret_event_id.as_str()
                        ]
                    },
                    "e1"
                ],
                [
                    "CalendarEvent/set",
                    {
                        "accountId": fixture.shared_account_id.as_str(),
                        "create": {
                            "new1": {
                                "calendarIds": {
                                    fixture.fb_calendar_id.as_str(): true
                                },
                                "title": "Forbidden Event",
                                "start": "2026-08-01T12:00:00"
                            }
                        }
                    },
                    "e2"
                ]
            ]
        }),
    );

    let event_get_resp = &event_response["methodResponses"][0][1];
    let event_list = event_get_resp["list"].as_array().expect("event list");
    assert_eq!(event_list.len(), 2);
    assert_eq!(event_list[0]["title"], "Team Meeting");
    assert!(event_list[1]["title"].is_null());

    let event_not_found = event_get_resp["notFound"]
        .as_array()
        .expect("event notFound");
    assert!(event_not_found.contains(&json!(fixture.secret_event_id.as_str())));

    let event_set_resp = &event_response["methodResponses"][1][1];
    assert_eq!(
        event_set_resp["notCreated"]["new1"]["type"],
        json!("forbidden")
    );
}

#[test]
fn address_book_share_with_invalid_permission_is_rejected() {
    let fixture = delegation_fixture(false);

    let response = api(
        &fixture.server,
        Some("Bearer owner-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:contacts"
            ],
            "methodCalls": [
                [
                    "AddressBook/set",
                    {
                        "accountId": fixture.shared_account_id.as_str(),
                        "create": {
                            "ab1": {
                                "name": "Invalid Share Book",
                                "shareWith": {
                                    fixture.grantee_principal_id.as_str(): {
                                        "mayRead": true,
                                        "mayWriteOwn": true
                                    }
                                }
                            }
                        }
                    },
                    "set1"
                ]
            ]
        }),
    );

    let set_resp = &response["methodResponses"][0][1];
    let not_created = &set_resp["notCreated"]["ab1"];
    assert_eq!(not_created["type"], "invalidProperties");
    assert_eq!(not_created["properties"], json!(["shareWith"]));
}

#[test]
fn principal_get_availability_honors_sharing_and_privacy() {
    let fixture = delegation_fixture(false);

    let response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:calendars",
                "urn:ietf:params:jmap:principals"
            ],
            "methodCalls": [
                [
                    "Principal/getAvailability",
                    {
                        "accountId": fixture.server.account_id().as_str(),
                        "id": fixture.owner_principal_id.as_str(),
                        "utcStart": "2026-08-01T00:00:00Z",
                        "utcEnd": "2026-08-01T23:59:59Z",
                        "showDetails": true
                    },
                    "a1"
                ],
                [
                    "Principal/getAvailability",
                    {
                        "accountId": fixture.server.account_id().as_str(),
                        "id": "unknown-principal",
                        "utcStart": "2026-08-01T00:00:00Z",
                        "utcEnd": "2026-08-01T23:59:59Z",
                        "showDetails": true
                    },
                    "a2"
                ]
            ]
        }),
    );

    let a1_resp = &response["methodResponses"][0][1];
    let periods = a1_resp["list"].as_array().expect("availability periods");

    // Normal and private events appear as busy; secret is completely hidden.
    assert_eq!(periods.len(), 2);

    // Period 1: normal event (10:00 to 11:00) with full details
    assert_eq!(periods[0]["utcStart"], "2026-08-01T10:00:00Z");
    assert_eq!(periods[0]["utcEnd"], "2026-08-01T11:00:00Z");
    assert_eq!(periods[0]["busyStatus"], "confirmed");
    assert_eq!(periods[0]["event"]["title"], "Team Meeting");

    // Period 2: private event (14:00 to 15:00) with title redacted
    assert_eq!(periods[1]["utcStart"], "2026-08-01T14:00:00Z");
    assert_eq!(periods[1]["utcEnd"], "2026-08-01T15:00:00Z");
    assert_eq!(periods[1]["busyStatus"], "confirmed");
    assert!(periods[1]["event"]["title"].is_null());

    // Unknown principal returns notFound error
    let a2_name = &response["methodResponses"][1][0];
    let a2_resp = &response["methodResponses"][1][1];
    assert_eq!(a2_name, "error");
    assert_eq!(a2_resp["type"], "notFound");
}
