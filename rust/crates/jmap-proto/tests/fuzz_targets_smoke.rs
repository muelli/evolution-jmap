// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable smoke tests for session, response, changes, and queryChanges fuzz targets.

use jmap_proto::error::{MethodError, RequestError, SetError};
use jmap_proto::methods::{ChangesResponse, QueryChangesResponse};
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
