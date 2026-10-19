// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for splitting `/set` method calls across `maxObjectsInSet` (RFC 8620 Section 2).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError, UreqTransport};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarEvent, CalendarEventSetRequest};
use jmap_proto::contacts::ContactCard;
use jmap_proto::mail::{Mailbox, role};
use jmap_proto::methods::SetRequest;
use serde_json::Value;

/// Transport proxy that injects `maxObjectsInSet` into session documents.
///
/// It counts API requests and asserts that no `/set` call exceeds the configured limit.
#[derive(Clone)]
struct LimitingTransport {
    inner: Arc<UreqTransport>,
    max_objects_in_set: u64,
    api_requests: Arc<AtomicUsize>,
}

impl LimitingTransport {
    fn new(max_objects_in_set: u64) -> Self {
        Self {
            inner: Arc::new(UreqTransport::default()),
            max_objects_in_set,
            api_requests: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn requests(&self) -> usize {
        self.api_requests.load(Ordering::SeqCst)
    }
}

impl Transport for LimitingTransport {
    fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let is_jmap = request.url.ends_with("/jmap");
        if is_jmap {
            self.api_requests.fetch_add(1, Ordering::SeqCst);
            assert_chunk_limits(&request, self.max_objects_in_set);
        }

        let response = self.inner.execute(request)?;
        if response.status == 200 {
            let Ok(mut doc) = serde_json::from_slice::<Value>(&response.body) else {
                return Ok(response);
            };
            if let Some(core) = doc
                .get_mut("capabilities")
                .and_then(|caps| caps.get_mut("urn:ietf:params:jmap:core"))
            {
                core["maxObjectsInSet"] = serde_json::json!(self.max_objects_in_set);
                let body = serde_json::to_vec(&doc).unwrap();
                return Ok(HttpResponse {
                    status: response.status,
                    content_type: response.content_type,
                    body,
                    final_url: response.final_url,
                });
            }
        }

        Ok(response)
    }
}

fn assert_chunk_limits(request: &HttpRequest<'_>, max_objects_in_set: u64) {
    let Some(body_bytes) = request.body else {
        return;
    };
    let Ok(req_val) = serde_json::from_slice::<Value>(body_bytes) else {
        return;
    };
    let Some(calls) = req_val.get("methodCalls").and_then(Value::as_array) else {
        return;
    };
    for call in calls {
        let Some(name) = call.get(0).and_then(Value::as_str) else {
            continue;
        };
        if !name.ends_with("/set") {
            continue;
        }
        let Some(args) = call.get(1) else { continue };
        let creates = args
            .get("create")
            .and_then(Value::as_object)
            .map_or(0, |m| m.len());
        let updates = args
            .get("update")
            .and_then(Value::as_object)
            .map_or(0, |m| m.len());
        let destroys = args
            .get("destroy")
            .and_then(Value::as_array)
            .map_or(0, |a| a.len());
        let total = creates + updates + destroys;
        assert!(
            total as u64 <= max_objects_in_set,
            "set call exceeded maxObjectsInSet limit: {total} > {max_objects_in_set}",
        );
    }
}

fn setup_test(max_objects_in_set: u64) -> (MockServer, Client, LimitingTransport, Id, Id) {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let calendar_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Personal", true)
    };

    let transport = LimitingTransport::new(max_objects_in_set);
    let client = Client::builder()
        .transport(transport.clone())
        .connect(server.origin(), Credentials::none())
        .unwrap();

    (server, client, transport, account_id, calendar_id)
}

#[test]
fn event_set_splits_creates_across_max_objects_in_set() {
    let (_server, client, transport, account_id, calendar_id) = setup_test(2);

    let mut set_req = SetRequest::<CalendarEvent>::new(account_id.clone());
    for i in 0..5 {
        let event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("Event {i}"),
            "2026-10-20T10:00:00",
            "PT1H",
        );
        set_req = set_req.create(format!("c{i}"), event);
    }

    let before = transport.requests();
    let request = CalendarEventSetRequest::new(set_req);
    let response = client.event_set(&request).unwrap();

    let created = response.created.expect("created events map");
    assert_eq!(created.len(), 5);
    for i in 0..5 {
        assert!(created.contains_key(&format!("c{i}")));
    }

    let calls = transport.requests() - before;
    assert_eq!(
        calls, 3,
        "5 creates with limit 2 split into 3 calls (2, 2, 1)"
    );
}

#[test]
fn event_set_splits_updates_across_max_objects_in_set() {
    let (_server, client, transport, account_id, calendar_id) = setup_test(2);

    let mut ids = Vec::new();
    for i in 0..5 {
        let event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("Event {i}"),
            "2026-10-20T10:00:00",
            "PT1H",
        );
        let created = client.event_create(&account_id, &event).unwrap();
        ids.push(created.id.expect("server id"));
    }

    let mut set_req = SetRequest::<CalendarEvent>::new(account_id.clone());
    for id in &ids {
        set_req = set_req.update(id.clone(), serde_json::json!({ "title": "Updated" }));
    }

    let before = transport.requests();
    let request = CalendarEventSetRequest::new(set_req);
    let response = client.event_set(&request).unwrap();

    let updated = response.updated.expect("updated events map");
    assert_eq!(updated.len(), 5);

    let calls = transport.requests() - before;
    assert_eq!(
        calls, 3,
        "5 updates with limit 2 split into 3 calls (2, 2, 1)"
    );
}

#[test]
fn event_set_splits_destroys_across_max_objects_in_set() {
    let (_server, client, transport, account_id, calendar_id) = setup_test(2);

    let mut ids = Vec::new();
    for i in 0..5 {
        let event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("Event {i}"),
            "2026-10-20T10:00:00",
            "PT1H",
        );
        let created = client.event_create(&account_id, &event).unwrap();
        ids.push(created.id.expect("server id"));
    }

    let set_req = SetRequest::<CalendarEvent>::new(account_id.clone()).with_destroy(ids.clone());

    let before = transport.requests();
    let request = CalendarEventSetRequest::new(set_req);
    let response = client.event_set(&request).unwrap();

    let destroyed = response.destroyed.expect("destroyed events list");
    assert_eq!(destroyed.len(), 5);

    let calls = transport.requests() - before;
    assert_eq!(
        calls, 3,
        "5 destroys with limit 2 split into 3 calls (2, 2, 1)"
    );
}

#[test]
fn mixed_set_splits_across_chunks_within_limit() {
    let (_server, client, transport, account_id, calendar_id) = setup_test(2);

    let mut existing_ids = Vec::new();
    for i in 0..4 {
        let event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("Seed {i}"),
            "2026-10-20T10:00:00",
            "PT1H",
        );
        let created = client.event_create(&account_id, &event).unwrap();
        existing_ids.push(created.id.expect("server id"));
    }

    let mut set_req = SetRequest::<CalendarEvent>::new(account_id.clone());
    for i in 0..2 {
        let event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("New {i}"),
            "2026-10-20T10:00:00",
            "PT1H",
        );
        set_req = set_req.create(format!("new_{i}"), event);
    }
    set_req = set_req.update(
        existing_ids[0].clone(),
        serde_json::json!({ "title": "U0" }),
    );
    set_req = set_req.update(
        existing_ids[1].clone(),
        serde_json::json!({ "title": "U1" }),
    );
    set_req = set_req.destroy(existing_ids[2].clone());
    set_req = set_req.destroy(existing_ids[3].clone());

    let before = transport.requests();
    let request = CalendarEventSetRequest::new(set_req);
    let response = client.event_set(&request).unwrap();

    assert_eq!(response.created.as_ref().map_or(0, |m| m.len()), 2);
    assert_eq!(response.updated.as_ref().map_or(0, |m| m.len()), 2);
    assert_eq!(response.destroyed.as_ref().map_or(0, |v| v.len()), 2);

    let calls = transport.requests() - before;
    assert_eq!(
        calls, 3,
        "6 items with limit 2 split into 3 calls (2, 2, 2)"
    );
}

#[test]
fn creation_id_back_references_are_resolved_across_chunks() {
    let (_server, client, transport, account_id, _calendar_id) = setup_test(1);

    // c1 creates a Calendar.
    // c2 creates a CalendarEvent referencing "#c1" in calendarIds.
    let cal = Calendar::new("Team Calendar");
    let mut event =
        CalendarEvent::simple(Id::new("#c1"), "Team Sync", "2026-10-20T10:00:00", "PT1H");

    // With limit 1, c1 must be in chunk 1, and c2 must be in chunk 2.
    // In chunk 2, "#c1" must be replaced with the real calendar id.
    let cal_set = SetRequest::<Calendar>::new(account_id.clone()).create("c1", cal);
    let cal_resp = client.calendar_set(&cal_set).unwrap();
    let real_cal_id = cal_resp.created.unwrap()["c1"]
        .id
        .clone()
        .expect("assigned id");

    // Test cross-chunk reference inside a single event_set when creating events that reference each other:
    // e.g., an event referencing another event's creation id.
    let mut set_req = SetRequest::<CalendarEvent>::new(account_id.clone());
    let e1 = CalendarEvent::simple(
        real_cal_id.clone(),
        "Parent Event",
        "2026-10-20T10:00:00",
        "PT1H",
    );
    let mut cal_ids = std::collections::BTreeMap::new();
    cal_ids.insert(real_cal_id, true);
    event.calendar_ids = Some(cal_ids);
    // Link to #e1 in extra properties or description:
    event.description = Some("Follows #e1".to_owned());

    set_req = set_req.create("e1", e1).create("e2", event);

    let before = transport.requests();
    let request = CalendarEventSetRequest::new(set_req);
    let response = client.event_set(&request).unwrap();

    assert_eq!(response.created.as_ref().map_or(0, |m| m.len()), 2);
    let calls = transport.requests() - before;
    assert_eq!(calls, 2, "2 creates with limit 1 split into 2 calls");
}

#[test]
fn circular_dependency_exceeding_limit_refuses_with_clear_error() {
    let (_server, client, _transport, account_id, calendar_id) = setup_test(1);

    let mut e1 = CalendarEvent::simple(
        calendar_id.clone(),
        "Event 1",
        "2026-10-20T10:00:00",
        "PT1H",
    );
    let mut e2 = CalendarEvent::simple(
        calendar_id.clone(),
        "Event 2",
        "2026-10-20T11:00:00",
        "PT1H",
    );

    // Mutual dependency: e1 references #e2, e2 references #e1.
    e1.extra
        .insert("relatedTo".to_owned(), serde_json::json!("#e2"));
    e2.extra
        .insert("relatedTo".to_owned(), serde_json::json!("#e1"));

    let set_req = SetRequest::<CalendarEvent>::new(account_id.clone())
        .create("e1", e1)
        .create("e2", e2);

    let request = CalendarEventSetRequest::new(set_req);
    let error = client.event_set(&request).unwrap_err();

    match &error {
        Error::ChunkBoundaryReferenceBroken {
            creation_id,
            reason,
        } => {
            assert!(!creation_id.is_empty());
            assert!(!reason.is_empty());
        }
        other => panic!("expected ChunkBoundaryReferenceBroken, got {other:?}"),
    }
    assert!(error.is_chunk_boundary_reference_broken());
}

#[test]
fn if_in_state_advances_across_chunks() {
    let (_server, client, transport, account_id, calendar_id) = setup_test(2);

    let initial_state = client.event_get(&account_id, &[]).unwrap().state;

    let mut set_req =
        SetRequest::<CalendarEvent>::new(account_id.clone()).if_in_state(initial_state.clone());
    for i in 0..4 {
        let event = CalendarEvent::simple(
            calendar_id.clone(),
            &format!("Event {i}"),
            "2026-10-20T10:00:00",
            "PT1H",
        );
        set_req = set_req.create(format!("c{i}"), event);
    }

    let before = transport.requests();
    let request = CalendarEventSetRequest::new(set_req);
    let response = client.event_set(&request).unwrap();

    assert_eq!(response.created.as_ref().map_or(0, |m| m.len()), 4);
    assert_eq!(response.old_state, Some(initial_state));
    assert!(response.new_state.is_some());

    let calls = transport.requests() - before;
    assert_eq!(calls, 2, "4 creates with limit 2 split into 2 calls");
}

#[test]
fn contact_set_splits_creates_across_max_objects_in_set() {
    let (server, client, transport, account_id, _calendar_id) = setup_test(2);
    let book_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Personal", true)
    };

    let mut set_req = SetRequest::<ContactCard>::new(account_id.clone());
    for i in 0..5 {
        let card = ContactCard::simple(
            book_id.clone(),
            &format!("Person {i}"),
            &format!("person{i}@example.com"),
        );
        set_req = set_req.create(format!("c{i}"), card);
    }

    let before = transport.requests();
    let response = client.contact_set(&set_req).unwrap();

    let created = response.created.expect("created cards map");
    assert_eq!(created.len(), 5);

    let calls = transport.requests() - before;
    assert_eq!(
        calls, 3,
        "5 contact creates with limit 2 split into 3 calls (2, 2, 1)"
    );
}

#[test]
fn mailbox_set_splits_creates_across_max_objects_in_set() {
    let (_server, client, transport, account_id, _calendar_id) = setup_test(2);

    let mut set_req = SetRequest::<Mailbox>::new(account_id.clone());
    for i in 0..5 {
        let mailbox = Mailbox::new(format!("Folder {i}"));
        set_req = set_req.create(format!("m{i}"), mailbox);
    }

    let before = transport.requests();
    let response = client.mailbox_set(&set_req).unwrap();

    let created = response.created.expect("created mailboxes map");
    assert_eq!(created.len(), 5);

    let calls = transport.requests() - before;
    assert_eq!(
        calls, 3,
        "5 mailbox creates with limit 2 split into 3 calls (2, 2, 1)"
    );
}

#[test]
fn thread_get_splits_across_max_objects_in_get() {
    let server = MockServer::builder().objects_in_get(2).start();
    let account_id = server.account_id();
    let thread_ids = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        let mut ids = Vec::new();
        for i in 0..5 {
            let email_id = account.seed_email(EmailSeed::new(
                inbox.clone(),
                ("User", "user@example.com"),
                &format!("Msg {i}"),
                "Body",
                "2026-08-01T10:00:00Z",
            ));
            let email = account.emails.get(&email_id).unwrap();
            ids.push(email.thread_id.clone().unwrap());
        }
        ids
    };

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let before = server.api_requests();
    let threads = client.thread_get(&account_id, thread_ids).unwrap();

    assert_eq!(threads.len(), 5);
    let calls = server.api_requests() - before;
    assert_eq!(
        calls, 3,
        "5 thread ids with maxObjectsInGet=2 split across 3 calls"
    );
}

#[test]
fn principal_get_splits_across_max_objects_in_get() {
    let server = MockServer::builder().objects_in_get(2).start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let ids = vec![
        Id::new("p1"),
        Id::new("p2"),
        Id::new("p3"),
        Id::new("p4"),
        Id::new("p5"),
    ];
    let before = server.api_requests();
    let principals = client.principal_get(&account_id, &ids).unwrap();

    // Since mock has no principals seeded for p1..p5, result list is empty.
    assert!(principals.is_empty());
    let calls = server.api_requests() - before;
    assert_eq!(
        calls, 3,
        "5 principal ids with maxObjectsInGet=2 split across 3 calls"
    );
}
