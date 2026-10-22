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

mod proptests {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use jmap_client::transport::{
        HttpRequest, HttpResponse, Transport, TransportError, UreqTransport,
    };
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::calendars::{CalendarEvent, CalendarEventSetRequest};
    use jmap_proto::methods::SetRequest;
    use proptest::prelude::*;
    use serde_json::{Value, json};

    #[derive(Clone, Debug)]
    struct RecordedChunk {
        created_ids: HashMap<String, String>,
        creates: BTreeMap<String, Value>,
        updates: BTreeMap<String, Value>,
        destroys: Vec<String>,
    }

    #[derive(Clone)]
    struct RecordingSetTransport {
        inner: Arc<UreqTransport>,
        max_objects_in_set: u64,
        fail_cids: HashSet<String>,
        chunks: Arc<Mutex<Vec<RecordedChunk>>>,
    }

    impl RecordingSetTransport {
        fn new(max_objects_in_set: u64, fail_cids: HashSet<String>) -> Self {
            Self {
                inner: Arc::new(UreqTransport::default()),
                max_objects_in_set,
                fail_cids,
                chunks: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn recorded_chunks(&self) -> Vec<RecordedChunk> {
            self.chunks.lock().unwrap().clone()
        }
    }

    impl Transport for RecordingSetTransport {
        fn execute(&self, request: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
            if request.url.ends_with("/jmap")
                && let Some(body_bytes) = request.body
                && let Ok(mut req_val) = serde_json::from_slice::<Value>(body_bytes)
            {
                let created_ids: HashMap<String, String> = req_val
                    .get("createdIds")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                            .collect()
                    })
                    .unwrap_or_default();

                let mut intercepted_not_created: BTreeMap<String, Value> = BTreeMap::new();
                let mut recorded_for_call = None;

                if let Some(calls) = req_val.get_mut("methodCalls").and_then(Value::as_array_mut) {
                    for call in calls.iter_mut() {
                        let is_set = call
                            .get(0)
                            .and_then(Value::as_str)
                            .is_some_and(|n| n.ends_with("/set"));
                        if !is_set {
                            continue;
                        }
                        if let Some(args) = call.get_mut(1).and_then(Value::as_object_mut) {
                            let creates: BTreeMap<String, Value> = args
                                .get("create")
                                .and_then(Value::as_object)
                                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                .unwrap_or_default();
                            let updates: BTreeMap<String, Value> = args
                                .get("update")
                                .and_then(Value::as_object)
                                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                .unwrap_or_default();
                            let destroys: Vec<String> = args
                                .get("destroy")
                                .and_then(Value::as_array)
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|v| v.as_str().map(str::to_owned))
                                        .collect()
                                })
                                .unwrap_or_default();

                            recorded_for_call = Some(RecordedChunk {
                                created_ids: created_ids.clone(),
                                creates,
                                updates,
                                destroys,
                            });

                            if !self.fail_cids.is_empty()
                                && let Some(create_obj) =
                                    args.get_mut("create").and_then(Value::as_object_mut)
                            {
                                for cid in &self.fail_cids {
                                    if create_obj.remove(cid).is_some() {
                                        intercepted_not_created.insert(
                                            cid.clone(),
                                            json!({
                                                "type": "invalidProperties",
                                                "description": "injected failure"
                                            }),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(rec) = recorded_for_call {
                    self.chunks.lock().unwrap().push(rec);
                }

                let modified_body = serde_json::to_vec(&req_val).unwrap();
                let modified_req = HttpRequest {
                    method: request.method,
                    url: request.url,
                    headers: request.headers,
                    body: Some(&modified_body),
                    cancel: request.cancel,
                    max_response_bytes: request.max_response_bytes,
                };
                let mut response = self.inner.execute(modified_req)?;
                if !intercepted_not_created.is_empty()
                    && let Ok(mut resp_val) = serde_json::from_slice::<Value>(&response.body)
                    && let Some(responses) = resp_val
                        .get_mut("methodResponses")
                        .and_then(Value::as_array_mut)
                {
                    for inv in responses.iter_mut() {
                        let is_set = inv
                            .get(0)
                            .and_then(Value::as_str)
                            .is_some_and(|n| n.ends_with("/set"));
                        if is_set && let Some(args) = inv.get_mut(1).and_then(Value::as_object_mut)
                        {
                            let entry = args.entry("notCreated").or_insert_with(|| json!({}));
                            if let Some(map) = entry.as_object_mut() {
                                for (k, v) in &intercepted_not_created {
                                    map.insert(k.clone(), v.clone());
                                }
                            }
                        }
                    }
                    response.body = serde_json::to_vec(&resp_val).unwrap();
                }
                return Ok(response);
            }

            let response = self.inner.execute(request)?;
            if response.status == 200
                && let Ok(mut doc) = serde_json::from_slice::<Value>(&response.body)
                && let Some(core) = doc
                    .get_mut("capabilities")
                    .and_then(|caps| caps.get_mut("urn:ietf:params:jmap:core"))
            {
                core["maxObjectsInSet"] = json!(self.max_objects_in_set);
                return Ok(HttpResponse {
                    status: response.status,
                    content_type: response.content_type,
                    body: serde_json::to_vec(&doc).unwrap(),
                    final_url: response.final_url,
                });
            }
            Ok(response)
        }
    }

    fn collect_refs_in_value(val: &Value, candidates: &HashSet<String>, out: &mut HashSet<String>) {
        match val {
            Value::String(s) => {
                if let Some(target) = s.strip_prefix('#').filter(|t| candidates.contains(*t)) {
                    out.insert(target.to_owned());
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    for seg in k.split('/') {
                        if let Some(target) =
                            seg.strip_prefix('#').filter(|t| candidates.contains(*t))
                        {
                            out.insert(target.to_owned());
                        }
                    }
                    collect_refs_in_value(v, candidates, out);
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    collect_refs_in_value(item, candidates, out);
                }
            }
            _ => {}
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::with_cases(32)
        })]

        #[test]
        fn prop_set_chunking_sends_every_object_once_and_never_leaks_unresolved_refs(
            limit in 1usize..=6,
            num_creates in 0usize..=14,
            num_updates in 0usize..=8,
            num_destroys in 0usize..=8,
            edge_seeds in prop::collection::vec((0usize..20, 0usize..20), 0..20),
            update_ref_seeds in prop::collection::vec(0usize..20, 0..10),
        ) {
            let server = MockServer::builder().start();
            let account_id = server.account_id();
            let (calendar_id, seeded_ids) = {
                let state = server.state();
                let mut state = state.lock().unwrap();
                let acct = state.account_mut(&account_id).unwrap();
                let cal_id = acct.seed_calendar("Personal", true);
                let mut ids = Vec::new();
                for i in 0..(num_updates + num_destroys) {
                    let id = acct.calendar_events.alloc_id();
                    let mut ev = CalendarEvent::simple(
                        cal_id.clone(),
                        &format!("Seed {i}"),
                        "2026-10-20T09:00:00",
                        "PT1H",
                    );
                    ev.id = Some(id.clone());
                    acct.calendar_events.seed_with_id(id.clone(), ev);
                    ids.push(id);
                }
                (cal_id, ids)
            };

            // Give creates non-monotonic names so BTreeMap order differs from topological order.
            let create_names: Vec<String> = (0..num_creates)
                .map(|i| format!("n{:02}_{}", (num_creates - i) % 7, i))
                .collect();
            let candidates: HashSet<String> = create_names.iter().cloned().collect();

            let mut set_req = SetRequest::<CalendarEvent>::new(account_id);
            for i in 0..num_creates {
                let mut ev = CalendarEvent::simple(
                    calendar_id.clone(),
                    &format!("Create {i}"),
                    "2026-10-20T10:00:00",
                    "PT1H",
                );
                // Add DAG references only to earlier indices (j < i) so the graph is acyclic.
                if i > 0 {
                    let mut refs_list = Vec::new();
                    for &(from, to) in &edge_seeds {
                        if from % num_creates == i {
                            let target_idx = to % i;
                            refs_list.push(format!("#{}", create_names[target_idx]));
                        }
                    }
                    if !refs_list.is_empty() {
                        ev.extra.insert("related".to_owned(), json!(refs_list));
                    }
                }
                set_req = set_req.create(create_names[i].clone(), ev);
            }

            let update_ids = &seeded_ids[..num_updates];
            let destroy_ids = &seeded_ids[num_updates..];

            for (u_idx, id) in update_ids.iter().enumerate() {
                let mut patch = json!({ "title": format!("Updated {u_idx}") });
                if num_creates > 0
                    && let Some(&seed) = update_ref_seeds.get(u_idx)
                {
                    let target = &create_names[seed % num_creates];
                    patch["description"] = json!(format!("#{target}"));
                }
                set_req = set_req.update(id.clone(), patch);
            }

            for id in destroy_ids {
                set_req = set_req.destroy(id.clone());
            }

            let transport = RecordingSetTransport::new(limit as u64, HashSet::new());
            let client = Client::builder()
                .transport(transport.clone())
                .connect(server.origin(), Credentials::none())
                .unwrap();

            let response = client
                .event_set(&CalendarEventSetRequest::new(set_req))
                .expect("acyclic set succeeds");

            let chunks = transport.recorded_chunks();
            let total_items = num_creates + num_updates + num_destroys;
            if total_items == 0 {
                prop_assert_eq!(chunks.len(), 1);
            } else {
                let mut seen_creates = HashSet::new();
                let mut seen_updates = HashSet::new();
                let mut seen_destroys = HashSet::new();

                for chunk in &chunks {
                    let count = chunk.creates.len() + chunk.updates.len() + chunk.destroys.len();
                    prop_assert!(count >= 1 && count <= limit);

                    let in_chunk: HashSet<String> = chunk.creates.keys().cloned().collect();

                    for (cid, val) in &chunk.creates {
                        prop_assert!(seen_creates.insert(cid.clone()), "duplicate create {cid}");
                        let mut refs = HashSet::new();
                        collect_refs_in_value(val, &candidates, &mut refs);
                        for r in refs {
                            prop_assert!(
                                in_chunk.contains(&r) || chunk.created_ids.contains_key(&r),
                                "create {cid} referenced #{r} across chunk boundary without createdIds"
                            );
                        }
                    }

                    for (uid, val) in &chunk.updates {
                        prop_assert!(seen_updates.insert(uid.clone()), "duplicate update {uid}");
                        let mut refs = HashSet::new();
                        collect_refs_in_value(val, &candidates, &mut refs);
                        for r in refs {
                            prop_assert!(
                                in_chunk.contains(&r) || chunk.created_ids.contains_key(&r),
                                "update {uid} referenced #{r} across chunk boundary without createdIds"
                            );
                        }
                    }

                    for did in &chunk.destroys {
                        prop_assert!(seen_destroys.insert(did.clone()), "duplicate destroy {did}");
                    }
                }

                prop_assert_eq!(seen_creates.len(), num_creates);
                prop_assert_eq!(seen_updates.len(), num_updates);
                prop_assert_eq!(seen_destroys.len(), num_destroys);
            }

            prop_assert_eq!(
                response.created.as_ref().map_or(0, |m| m.len()),
                num_creates
            );
            prop_assert_eq!(
                response.updated.as_ref().map_or(0, |m| m.len()),
                num_updates
            );
            prop_assert_eq!(
                response.destroyed.as_ref().map_or(0, |v| v.len()),
                num_destroys
            );
        }

        #[test]
        fn prop_set_chunking_with_partial_failures_never_sends_dangling_refs(
            limit in 1usize..=5,
            num_creates in 2usize..=12,
            edge_seeds in prop::collection::vec((0usize..20, 0usize..20), 1..20),
            fail_mask in prop::collection::vec(any::<bool>(), 2..=12),
        ) {
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

            // Give creates reversed names so BTreeMap order opposes dependency order.
            let create_names: Vec<String> = (0..num_creates)
                .map(|i| format!("c{:02}", num_creates - 1 - i))
                .collect();
            let candidates: HashSet<String> = create_names.iter().cloned().collect();

            let mut fail_cids = HashSet::new();
            for (i, &should_fail) in fail_mask.iter().take(num_creates).enumerate() {
                if should_fail {
                    fail_cids.insert(create_names[i].clone());
                }
            }

            let mut set_req = SetRequest::<CalendarEvent>::new(account_id);
            for i in 0..num_creates {
                let mut ev = CalendarEvent::simple(
                    calendar_id.clone(),
                    &format!("Create {i}"),
                    "2026-10-20T10:00:00",
                    "PT1H",
                );
                if i > 0 {
                    let mut refs_list = Vec::new();
                    for &(from, to) in &edge_seeds {
                        if from % num_creates == i {
                            let target_idx = to % i;
                            refs_list.push(format!("#{}", create_names[target_idx]));
                        }
                    }
                    if !refs_list.is_empty() {
                        ev.extra.insert("related".to_owned(), json!(refs_list));
                    }
                }
                set_req = set_req.create(create_names[i].clone(), ev);
            }

            let transport = RecordingSetTransport::new(limit as u64, fail_cids);
            let client = Client::builder()
                .transport(transport.clone())
                .connect(server.origin(), Credentials::none())
                .unwrap();

            let response = client
                .event_set(&CalendarEventSetRequest::new(set_req))
                .expect("set with per-item failures returns Ok(SetResponse)");

            let chunks = transport.recorded_chunks();
            let mut seen_creates = HashSet::new();
            for chunk in &chunks {
                let count = chunk.creates.len() + chunk.updates.len() + chunk.destroys.len();
                prop_assert!(count >= 1 && count <= limit);

                let in_chunk: HashSet<String> = chunk.creates.keys().cloned().collect();
                for (cid, val) in &chunk.creates {
                    prop_assert!(seen_creates.insert(cid.clone()), "duplicate create {cid}");
                    let mut refs = HashSet::new();
                    collect_refs_in_value(val, &candidates, &mut refs);
                    for r in refs {
                        prop_assert!(
                            in_chunk.contains(&r) || chunk.created_ids.contains_key(&r),
                            "create {cid} referenced #{r} which is neither in chunk ({in_chunk:?}) nor in createdIds ({:?})",
                            chunk.created_ids
                        );
                    }
                }
            }

            let total_accounted = response.created.as_ref().map_or(0, |m| m.len())
                + response.not_created.as_ref().map_or(0, |m| m.len());
            prop_assert_eq!(total_accounted, num_creates);
        }

        #[test]
        fn prop_set_chunking_rejects_cycles_exceeding_limit(
            limit in 1usize..=5,
            extra_cycle_nodes in 1usize..=4,
        ) {
            let cycle_len = limit + extra_cycle_nodes;
            let (_server, client, transport, account_id, calendar_id) = super::setup_test(limit as u64);

            let mut set_req = SetRequest::<CalendarEvent>::new(account_id);
            for i in 0..cycle_len {
                let next = (i + 1) % cycle_len;
                let mut ev = CalendarEvent::simple(
                    calendar_id.clone(),
                    &format!("Cycle {i}"),
                    "2026-10-20T10:00:00",
                    "PT1H",
                );
                ev.extra.insert("related".to_owned(), json!(format!("#c{next}")));
                set_req = set_req.create(format!("c{i}"), ev);
            }

            let before = transport.requests();
            let err = client
                .event_set(&CalendarEventSetRequest::new(set_req))
                .expect_err("cycle exceeding limit must be rejected");
            prop_assert!(err.is_chunk_boundary_reference_broken());
            prop_assert_eq!(transport.requests() - before, 0);
        }
    }
}
