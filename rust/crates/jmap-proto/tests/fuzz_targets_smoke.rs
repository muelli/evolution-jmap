// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable smoke tests for session, response, changes, and queryChanges fuzz targets.

use jmap_proto::error::{MethodError, RequestError, SetError};
use jmap_proto::methods::{
    AddedItem, ChangesRequest, ChangesResponse, QueryChangesRequest, QueryChangesResponse,
};
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use jmap_proto::session::Session;

#[test]
fn hostile_bytes_do_not_panic_session_or_response_or_changes() {
    let hostile_inputs: &[&[u8]] = &[
        b"",
        b"{",
        b"[",
        b"}",
        b"]",
        b"null",
        b"true",
        b"12345",
        b"\"unterminated string",
        b"{\"methodResponses\": null}",
        b"{\"methodResponses\": [\"invalid\"]}",
        b"{\"sessionState\": 12345}",
        b"\xFF\xFE\x00\x00",
        b"{\"accountId\": null, \"oldState\": 123}",
        b"{\"accountId\": \"acc\", \"oldQueryState\": false}",
    ];

    for input in hostile_inputs {
        if let Ok(session) = serde_json::from_slice::<Session>(input) {
            let _ = serde_json::to_vec(&session);
        }
        if let Ok(resp) = serde_json::from_slice::<Response>(input) {
            let _ = serde_json::to_vec(&resp);
        }
        if let Ok(changes) = serde_json::from_slice::<ChangesResponse>(input) {
            let _ = serde_json::to_vec(&changes);
        }
        if let Ok(qc) = serde_json::from_slice::<QueryChangesResponse>(input) {
            let _ = serde_json::to_vec(&qc);
        }
        if let Ok(req) = serde_json::from_slice::<ChangesRequest>(input) {
            let _ = serde_json::to_vec(&req);
        }
        if let Ok(qreq) = serde_json::from_slice::<QueryChangesRequest<serde_json::Value>>(input) {
            let _ = serde_json::to_vec(&qreq);
        }
        if let Ok(item) = serde_json::from_slice::<AddedItem>(input) {
            let _ = serde_json::to_vec(&item);
        }
        if let Ok(inv) = serde_json::from_slice::<Invocation>(input) {
            let _ = inv.is_error();
            let _ = inv.parse::<MethodError>();
            let _ = serde_json::to_vec(&inv);
        }
        if let Ok(req) = serde_json::from_slice::<Request>(input) {
            let _ = serde_json::to_vec(&req);
        }
        if let Ok(rref) = serde_json::from_slice::<ResultReference>(input) {
            let _ = serde_json::to_vec(&rref);
        }
        if let Ok(err) = serde_json::from_slice::<RequestError>(input) {
            let _ = serde_json::to_vec(&err);
        }
        if let Ok(err) = serde_json::from_slice::<MethodError>(input) {
            let _ = serde_json::to_vec(&err);
        }
        if let Ok(err) = serde_json::from_slice::<SetError>(input) {
            let _ = serde_json::to_vec(&err);
        }
    }
}

#[test]
fn changes_and_query_changes_seeds_roundtrip() {
    let changes_json = br#"{
        "accountId": "acc1",
        "oldState": "state1",
        "newState": "state2",
        "hasMoreChanges": false,
        "created": ["id1", "id2"],
        "updated": ["id3"],
        "destroyed": ["id4"]
    }"#;
    let chg: ChangesResponse =
        serde_json::from_slice(changes_json).expect("ChangesResponse parses");
    assert_eq!(chg.account_id.as_str(), "acc1");
    let serialized = serde_json::to_vec(&chg).expect("ChangesResponse serializes");
    assert!(serde_json::from_slice::<ChangesResponse>(&serialized).is_ok());

    let query_changes_json = br#"{
        "accountId": "acc1",
        "oldQueryState": "qs1",
        "newQueryState": "qs2",
        "total": 5,
        "removed": ["id1"],
        "added": [{"id": "id2", "index": 0}]
    }"#;
    let qc: QueryChangesResponse =
        serde_json::from_slice(query_changes_json).expect("QueryChangesResponse parses");
    assert_eq!(qc.account_id.as_str(), "acc1");
    let serialized_qc = serde_json::to_vec(&qc).expect("QueryChangesResponse serializes");
    assert!(serde_json::from_slice::<QueryChangesResponse>(&serialized_qc).is_ok());
}

#[test]
fn changes_and_query_changes_requests_smoke() {
    let changes_request_json = br#"{
        "accountId": "acc1",
        "sinceState": "state1",
        "maxChanges": 100
    }"#;
    let req: ChangesRequest =
        serde_json::from_slice(changes_request_json).expect("ChangesRequest parses");
    assert_eq!(req.account_id.as_str(), "acc1");
    assert_eq!(req.since_state.as_str(), "state1");
    assert_eq!(req.max_changes, Some(100));

    let built_req =
        ChangesRequest::new(req.account_id.clone(), req.since_state.clone()).max_changes(50);
    assert_eq!(built_req.max_changes, Some(50));
    let serialized_req = serde_json::to_vec(&req).expect("ChangesRequest serializes");
    assert!(serde_json::from_slice::<ChangesRequest>(&serialized_req).is_ok());

    let query_changes_request_json = br#"{
        "accountId": "acc1",
        "sinceQueryState": "qstate1",
        "maxChanges": 25,
        "upToId": "id42",
        "calculateTotal": true,
        "collapseThreads": true
    }"#;
    let qreq: QueryChangesRequest<serde_json::Value> =
        serde_json::from_slice(query_changes_request_json).expect("QueryChangesRequest parses");
    assert_eq!(qreq.account_id.as_str(), "acc1");
    assert_eq!(qreq.since_query_state.as_str(), "qstate1");
    assert_eq!(qreq.max_changes, Some(25));
    assert_eq!(qreq.up_to_id.as_ref().map(|id| id.as_str()), Some("id42"));
    assert!(qreq.calculate_total);
    assert!(qreq.collapse_threads);

    let built_qreq: QueryChangesRequest<serde_json::Value> =
        QueryChangesRequest::new(qreq.account_id.clone(), qreq.since_query_state.clone())
            .max_changes(10)
            .up_to_id("id10")
            .calculate_total()
            .collapse_threads();
    assert_eq!(built_qreq.max_changes, Some(10));
    assert!(built_qreq.calculate_total);
    let serialized_qreq = serde_json::to_vec(&qreq).expect("QueryChangesRequest serializes");
    assert!(
        serde_json::from_slice::<QueryChangesRequest<serde_json::Value>>(&serialized_qreq).is_ok()
    );

    let item_json = br#"{"id": "item1", "index": 3}"#;
    let item: AddedItem = serde_json::from_slice(item_json).expect("AddedItem parses");
    assert_eq!(item.id.as_str(), "item1");
    assert_eq!(item.index, 3);
    let built_item = AddedItem::new("item2", 4);
    assert_eq!(built_item.id.as_str(), "item2");
    assert_eq!(built_item.index, 4);
    let serialized_item = serde_json::to_vec(&item).expect("AddedItem serializes");
    assert!(serde_json::from_slice::<AddedItem>(&serialized_item).is_ok());
}

#[test]
fn response_and_invocation_and_errors_smoke() {
    let response_json = br##"{
        "methodResponses": [
            ["Core/echo", {"arg": "value"}, "c1"],
            ["error", {"type": "unknownMethod", "description": "not found"}, "c2"]
        ],
        "createdIds": {"#1": "real-id"},
        "sessionState": "state1"
    }"##;
    let resp: Response = serde_json::from_slice(response_json).expect("Response parses");
    assert_eq!(resp.responses_for("c1").count(), 1);
    assert_eq!(resp.responses_for("c2").count(), 1);
    assert_eq!(resp.responses_for("c3").count(), 0);

    for invocation in &resp.method_responses {
        if invocation.is_error() {
            let err: MethodError = invocation.parse().expect("MethodError parses");
            assert_eq!(err.error_type, "unknownMethod");
            assert_eq!(err.description.as_deref(), Some("not found"));
        }
    }

    let serialized = serde_json::to_vec(&resp).expect("Response serializes");
    let resp2: Response = serde_json::from_slice(&serialized).expect("Response roundtrips");
    assert_eq!(resp, resp2);

    let req_err_json =
        br#"{"type": "urn:ietf:params:jmap:error:limit", "status": 400, "limit": "maxSizeRequest"}"#;
    let req_err: RequestError = serde_json::from_slice(req_err_json).expect("RequestError parses");
    assert_eq!(req_err.status, Some(400));
    assert_eq!(req_err.error_type, "urn:ietf:params:jmap:error:limit");
    let serialized_req = serde_json::to_vec(&req_err).expect("RequestError serializes");
    assert!(serde_json::from_slice::<RequestError>(&serialized_req).is_ok());

    let set_err_json = br#"{"type": "invalidProperties", "properties": ["name"]}"#;
    let set_err: SetError = serde_json::from_slice(set_err_json).expect("SetError parses");
    assert_eq!(
        set_err.properties.as_deref(),
        Some(&["name".to_string()][..])
    );
    let serialized_set = serde_json::to_vec(&set_err).expect("SetError serializes");
    assert!(serde_json::from_slice::<SetError>(&serialized_set).is_ok());

    let req_json =
        br#"{"using": ["urn:ietf:params:jmap:core"], "methodCalls": [["Core/echo", {}, "c1"]]}"#;
    let req: Request = serde_json::from_slice(req_json).expect("Request parses");
    assert_eq!(req.method_calls.len(), 1);
    let serialized_request = serde_json::to_vec(&req).expect("Request serializes");
    assert!(serde_json::from_slice::<Request>(&serialized_request).is_ok());

    let ref_json = br#"{"resultOf": "c1", "name": "ids", "path": "/0"}"#;
    let rref: ResultReference = serde_json::from_slice(ref_json).expect("ResultReference parses");
    assert_eq!(rref.result_of, "c1");
    let serialized_ref = serde_json::to_vec(&rref).expect("ResultReference serializes");
    assert!(serde_json::from_slice::<ResultReference>(&serialized_ref).is_ok());
}

#[test]
fn hostile_bytes_do_not_panic_expanded_proto_parsers() {
    let hostile_inputs: &[&[u8]] = &[
        b"",
        b"{",
        b"[",
        b"}",
        b"]",
        b"null",
        b"true",
        b"12345",
        b"\"unterminated string",
        b"{\"name\": null}",
        b"{\"values\": [1, 2, 3]}",
        b"mailto:user@example.com?body=^n^n^",
        b"https://example.com/path?arg=val#fragment",
        b"\xFF\xFE\x00\x00",
        b"This is an overlong text line that needs folding into multiple 75-octet segments without split.",
    ];

    for input in hostile_inputs {
        if let Ok(ab) = serde_json::from_slice::<jmap_proto::contacts::AddressBook>(input) {
            let _ = serde_json::to_vec(&ab);
        }
        if let Ok(cal) = serde_json::from_slice::<jmap_proto::calendars::Calendar>(input) {
            let _ = serde_json::to_vec(&cal);
        }
        if let Ok(pi) = serde_json::from_slice::<jmap_proto::calendars::ParticipantIdentity>(input)
        {
            let _ = serde_json::to_vec(&pi);
        }
        if let Ok(cen) =
            serde_json::from_slice::<jmap_proto::calendars::CalendarEventNotification>(input)
        {
            let _ = serde_json::to_vec(&cen);
        }
        if let Ok(task) = serde_json::from_slice::<jmap_proto::tasks::Task>(input) {
            let _ = serde_json::to_vec(&task);
        }
        if let Ok(tl) = serde_json::from_slice::<jmap_proto::tasks::TaskList>(input) {
            let _ = serde_json::to_vec(&tl);
        }
        if let Ok(pr) = serde_json::from_slice::<jmap_proto::principals::Principal>(input) {
            let _ = serde_json::to_vec(&pr);
        }
        if let Ok(sn) = serde_json::from_slice::<jmap_proto::principals::ShareNotification>(input) {
            let _ = serde_json::to_vec(&sn);
        }
        if let Ok(identity) = serde_json::from_slice::<jmap_proto::mail::Identity>(input) {
            let _ = serde_json::to_vec(&identity);
        }
        if let Ok(sub) = serde_json::from_slice::<jmap_proto::mail::EmailSubmission>(input) {
            let _ = serde_json::to_vec(&sub);
        }
        if let Ok(th) = serde_json::from_slice::<jmap_proto::mail::Thread>(input) {
            let _ = serde_json::to_vec(&th);
        }
        if let Ok(snip) = serde_json::from_slice::<jmap_proto::mail::SearchSnippet>(input) {
            let _ = serde_json::to_vec(&snip);
        }
        if let Ok(script) = serde_json::from_slice::<jmap_proto::sieve::SieveScript>(input) {
            let _ = serde_json::to_vec(&script);
        }
        if let Ok(quota) = serde_json::from_slice::<jmap_proto::quota::Quota>(input) {
            let _ = serde_json::to_vec(&quota);
        }
        if let Ok(fn_node) = serde_json::from_slice::<jmap_proto::filenode::FileNode>(input) {
            let _ = serde_json::to_vec(&fn_node);
        }
        if let Ok(blob_up) = serde_json::from_slice::<jmap_proto::blob::BlobUploadResponse>(input) {
            let _ = serde_json::to_vec(&blob_up);
        }
        if let Ok(blob_get) = serde_json::from_slice::<jmap_proto::blob::BlobGetResponse>(input) {
            let _ = serde_json::to_vec(&blob_get);
        }
        if let Ok(ps) = serde_json::from_slice::<jmap_proto::push::PushSubscription>(input) {
            let _ = serde_json::to_vec(&ps);
        }
        if let Ok(sc) = serde_json::from_slice::<jmap_proto::push::StateChange>(input) {
            let _ = serde_json::to_vec(&sc);
        }
        if let Ok(ws_req) = serde_json::from_slice::<jmap_proto::websocket::WebSocketRequest>(input)
        {
            let _ = serde_json::to_vec(&ws_req);
        }
        if let Ok(ws_resp) =
            serde_json::from_slice::<jmap_proto::websocket::WebSocketResponse>(input)
        {
            let _ = serde_json::to_vec(&ws_resp);
        }
        if let Ok(s) = std::str::from_utf8(input) {
            let _ = jmap_proto::text::caret_decoded(s);
            let _ = jmap_proto::text::caret_encoded(s);
            let _ = jmap_proto::text::split_uri(s);
            let _ = jmap_proto::text::fold_overlong_lines(s.to_string());
            let _ = jmap_proto::text::is_valid_scheme(s);
        }
    }
}
