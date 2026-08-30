// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::id::Id;
use jmap_proto::request::Invocation;
use jmap_proto::websocket::{
    SUBPROTOCOL, WebSocketPushDisable, WebSocketPushEnable, WebSocketRequest,
    WebSocketRequestError, WebSocketResponse, message_type,
};
use serde_json::json;

#[test]
fn websocket_subprotocol_constant() {
    assert_eq!(SUBPROTOCOL, "jmap");
    assert_eq!(message_type::REQUEST, "Request");
    assert_eq!(message_type::RESPONSE, "Response");
    assert_eq!(message_type::REQUEST_ERROR, "RequestError");
    assert_eq!(message_type::PUSH_ENABLE, "WebSocketPushEnable");
    assert_eq!(message_type::PUSH_DISABLE, "WebSocketPushDisable");
    assert_eq!(message_type::STATE_CHANGE, "StateChange");
}

#[test]
fn websocket_request_builder_and_roundtrip() {
    let req = WebSocketRequest::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
        .with_id("req-123")
        .call("Mailbox/get", &json!({"accountId": "acc1"}), "c1")
        .expect("serialization")
        .with_created_ids([(Id::from("k1"), Id::from("v1"))].into_iter().collect());

    let serialized = serde_json::to_string(&req).expect("serialize");
    let val: serde_json::Value = serde_json::from_str(&serialized).expect("parse json");

    assert_eq!(val["@type"], "Request");
    assert_eq!(val["id"], "req-123");
    assert_eq!(val["using"][0], "urn:ietf:params:jmap:core");
    assert_eq!(val["using"][1], "urn:ietf:params:jmap:mail");
    assert_eq!(val["methodCalls"][0][0], "Mailbox/get");
    assert_eq!(val["methodCalls"][0][2], "c1");
    assert_eq!(val["createdIds"]["k1"], "v1");

    let parsed: WebSocketRequest = serde_json::from_str(&serialized).expect("deserialize");
    assert_eq!(parsed, req);
}

#[test]
fn websocket_response_builder_and_roundtrip() {
    let invocation = Invocation::new(
        "Mailbox/get",
        &json!({"accountId": "acc1", "state": "s1"}),
        "c1",
    )
    .expect("invocation");
    let resp = WebSocketResponse::new()
        .with_id("req-123")
        .with_session_state("state-token-xyz")
        .with_method_response(invocation.clone())
        .with_created_ids([(Id::from("k1"), Id::from("v1"))].into_iter().collect());

    let serialized = serde_json::to_string(&resp).expect("serialize");
    let val: serde_json::Value = serde_json::from_str(&serialized).expect("parse json");

    assert_eq!(val["@type"], "Response");
    assert_eq!(val["id"], "req-123");
    assert_eq!(val["sessionState"], "state-token-xyz");
    assert_eq!(val["methodResponses"][0][0], "Mailbox/get");
    assert_eq!(val["createdIds"]["k1"], "v1");

    let parsed: WebSocketResponse = serde_json::from_str(&serialized).expect("deserialize");
    assert_eq!(parsed, resp);
    assert_eq!(parsed.responses_for("c1").count(), 1);
    assert_eq!(parsed.responses_for("nonexistent").count(), 0);
}

#[test]
fn websocket_request_error_builder_and_roundtrip() {
    let err = WebSocketRequestError::new("urn:ietf:params:jmap:error:limit")
        .with_id("req-123")
        .with_status(400)
        .with_detail("Request exceeds maxCallsInRequest limit");

    let serialized = serde_json::to_string(&err).expect("serialize");
    let val: serde_json::Value = serde_json::from_str(&serialized).expect("parse json");

    assert_eq!(val["@type"], "RequestError");
    assert_eq!(val["id"], "req-123");
    assert_eq!(val["type"], "urn:ietf:params:jmap:error:limit");
    assert_eq!(val["status"], 400);
    assert_eq!(val["detail"], "Request exceeds maxCallsInRequest limit");

    let parsed: WebSocketRequestError = serde_json::from_str(&serialized).expect("deserialize");
    assert_eq!(parsed, err);
}

#[test]
fn websocket_push_enable_and_disable_roundtrip() {
    let enable_all = WebSocketPushEnable::new();
    let val_all = serde_json::to_value(&enable_all).expect("to_value");
    assert_eq!(val_all["@type"], "WebSocketPushEnable");
    assert!(val_all.get("dataTypes").is_none());

    let enable_filtered = WebSocketPushEnable::new().with_data_types(["Email", "Mailbox"]);
    let val_filtered = serde_json::to_value(&enable_filtered).expect("to_value");
    assert_eq!(val_filtered["@type"], "WebSocketPushEnable");
    assert_eq!(val_filtered["dataTypes"][0], "Email");
    assert_eq!(val_filtered["dataTypes"][1], "Mailbox");

    let parsed_enable: WebSocketPushEnable =
        serde_json::from_value(val_filtered).expect("deserialize");
    assert_eq!(parsed_enable, enable_filtered);

    let disable = WebSocketPushDisable::new();
    let val_disable = serde_json::to_value(&disable).expect("to_value");
    assert_eq!(val_disable["@type"], "WebSocketPushDisable");

    let parsed_disable: WebSocketPushDisable =
        serde_json::from_value(val_disable).expect("deserialize");
    assert_eq!(parsed_disable, disable);
}

#[test]
fn websocket_forward_compatibility_preserves_unknown_members() {
    let wire_req = json!({
        "@type": "Request",
        "id": "req-99",
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [],
        "customVendorTraceId": "trace-777",
        "priority": "high"
    });
    let req: WebSocketRequest = serde_json::from_value(wire_req).expect("deserialize");
    assert_eq!(req.id.as_deref(), Some("req-99"));
    assert_eq!(
        req.extra.get("customVendorTraceId"),
        Some(&json!("trace-777"))
    );
    assert_eq!(req.extra.get("priority"), Some(&json!("high")));

    let wire_resp = json!({
        "@type": "Response",
        "id": "req-99",
        "methodResponses": [],
        "sessionState": "state-1",
        "serverProcessingTimeMs": 42
    });
    let resp: WebSocketResponse = serde_json::from_value(wire_resp).expect("deserialize");
    assert_eq!(resp.id.as_deref(), Some("req-99"));
    assert_eq!(resp.extra.get("serverProcessingTimeMs"), Some(&json!(42)));

    let wire_err = json!({
        "@type": "RequestError",
        "type": "urn:ietf:params:jmap:error:unknownCapability",
        "status": 400,
        "detail": "Unknown URN",
        "unsupportedCapabilities": ["urn:custom:future:cap"]
    });
    let err: WebSocketRequestError = serde_json::from_value(wire_err).expect("deserialize");
    assert_eq!(
        err.error_type,
        "urn:ietf:params:jmap:error:unknownCapability"
    );
    assert_eq!(
        err.extra.get("unsupportedCapabilities"),
        Some(&json!(["urn:custom:future:cap"]))
    );
}
