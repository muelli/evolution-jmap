// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::id::Id;
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use jmap_proto::state::State;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EchoArguments {
    message: String,
    count: u32,
}

#[test]
fn request_new_and_call_builder_chaining() {
    let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
        .call(
            "Core/echo",
            &EchoArguments {
                message: "ping".to_string(),
                count: 42,
            },
            "c1",
        )
        .expect("serialization should succeed")
        .call("Mailbox/get", &json!({"accountId": "acc1"}), "c2")
        .expect("serialization should succeed");

    assert_eq!(
        request.using,
        vec![
            "urn:ietf:params:jmap:core".to_string(),
            "urn:ietf:params:jmap:mail".to_string()
        ]
    );
    assert_eq!(request.method_calls.len(), 2);

    let first = &request.method_calls[0];
    assert_eq!(first.name, "Core/echo");
    assert_eq!(first.call_id, "c1");
    assert_eq!(first.arguments, json!({"message": "ping", "count": 42}));
    assert!(!first.is_error());

    let second = &request.method_calls[1];
    assert_eq!(second.name, "Mailbox/get");
    assert_eq!(second.call_id, "c2");
    assert_eq!(second.arguments, json!({"accountId": "acc1"}));
    assert!(!second.is_error());
}

#[test]
fn request_with_created_ids_and_serde_roundtrip() {
    let mut created_ids = BTreeMap::new();
    created_ids.insert(Id::from("temp_client_1"), Id::from("server_id_100"));
    created_ids.insert(Id::from("temp_client_2"), Id::from("server_id_200"));

    let request = Request::new(["urn:ietf:params:jmap:core"])
        .with_created_ids(created_ids.clone())
        .call("Core/echo", &json!({"hello": "world"}), "call_0")
        .expect("call serialization");

    assert_eq!(request.created_ids.as_ref(), Some(&created_ids));

    let serialized = serde_json::to_value(&request).expect("serialize request");
    assert_eq!(serialized["using"], json!(["urn:ietf:params:jmap:core"]));
    assert_eq!(
        serialized["createdIds"]["temp_client_1"],
        json!("server_id_100")
    );
    assert_eq!(
        serialized["createdIds"]["temp_client_2"],
        json!("server_id_200")
    );
    assert_eq!(
        serialized["methodCalls"][0],
        json!(["Core/echo", {"hello": "world"}, "call_0"])
    );

    let deserialized: Request = serde_json::from_value(serialized).expect("deserialize request");
    assert_eq!(deserialized, request);

    // Without created_ids, field is omitted in serialization
    let minimal = Request::new(["urn:ietf:params:jmap:core"]);
    let min_value = serde_json::to_value(&minimal).expect("serialize minimal request");
    assert!(min_value.get("createdIds").is_none());
}

#[test]
fn invocation_constructors_parse_and_error_predicate() {
    let args = EchoArguments {
        message: "hello".to_string(),
        count: 7,
    };
    let inv1 = Invocation::new("Core/echo", &args, "0").expect("Invocation::new");
    assert_eq!(inv1.name, "Core/echo");
    assert_eq!(inv1.call_id, "0");
    assert_eq!(inv1.arguments, json!({"message": "hello", "count": 7}));
    assert!(!inv1.is_error());

    let parsed: EchoArguments = inv1.parse().expect("Invocation::parse");
    assert_eq!(parsed, args);

    // Mismatched schema returns error on parse
    let parse_err: Result<Vec<String>, _> = inv1.parse();
    assert!(parse_err.is_err());

    // from_value constructor
    let raw_val = json!({"type": "notFound", "description": "Item does not exist"});
    let inv2 = Invocation::from_value("error", raw_val.clone(), "err_call");
    assert_eq!(inv2.name, "error");
    assert_eq!(inv2.arguments, raw_val);
    assert_eq!(inv2.call_id, "err_call");
    assert!(inv2.is_error());

    // Edge cases for is_error: must be exactly "error"
    let inv_prefix = Invocation::from_value("error/sub", json!({}), "1");
    assert!(!inv_prefix.is_error());
    let inv_empty = Invocation::from_value("", json!({}), "2");
    assert!(!inv_empty.is_error());
}

#[test]
fn invocation_wire_serialization_and_deserialization() {
    let inv = Invocation::from_value("Mailbox/get", json!({"ids": ["m1", "m2"]}), "inv_42");

    let wire_json = serde_json::to_value(&inv).expect("serialize invocation");
    assert_eq!(
        wire_json,
        json!(["Mailbox/get", {"ids": ["m1", "m2"]}, "inv_42"])
    );

    let restored: Invocation = serde_json::from_value(wire_json).expect("deserialize invocation");
    assert_eq!(restored, inv);

    // Malformed array representations fail deserialization
    let wrong_len = json!(["Mailbox/get", {}]);
    assert!(serde_json::from_value::<Invocation>(wrong_len).is_err());

    let wrong_types = json!([123, {}, "call_id"]);
    assert!(serde_json::from_value::<Invocation>(wrong_types).is_err());
}

#[test]
fn result_reference_constructors_and_serde() {
    let result_ref = ResultReference::new("call_0", "Mailbox/query", "/ids");
    assert_eq!(result_ref.result_of, "call_0");
    assert_eq!(result_ref.name, "Mailbox/query");
    assert_eq!(result_ref.path, "/ids");

    let serialized = serde_json::to_value(&result_ref).expect("serialize ResultReference");
    assert_eq!(
        serialized,
        json!({
            "resultOf": "call_0",
            "name": "Mailbox/query",
            "path": "/ids"
        })
    );

    let restored: ResultReference =
        serde_json::from_value(serialized).expect("deserialize ResultReference");
    assert_eq!(restored, result_ref);
}

#[test]
fn response_constructors_and_responses_for_filtering() {
    let inv1 = Invocation::from_value("Mailbox/get", json!({"list": [{"id": "m1"}]}), "c1");
    let inv2 = Invocation::from_value("Email/get", json!({"list": [{"id": "e1"}]}), "c2");
    let inv3 = Invocation::from_value("Mailbox/get", json!({"notFound": ["m99"]}), "c1");

    let mut created = BTreeMap::new();
    created.insert(Id::from("temp_draft"), Id::from("email_12345"));

    let response = Response::new(State::from("state_token_777"))
        .with_method_response(inv1.clone())
        .with_method_response(inv2.clone())
        .with_method_response(inv3.clone())
        .with_created_ids(created.clone());

    assert_eq!(response.session_state, State::from("state_token_777"));
    assert_eq!(response.created_ids.as_ref(), Some(&created));
    assert_eq!(response.method_responses.len(), 3);

    // Filter responses for call_id "c1" (expect inv1 and inv3 in order)
    let c1_responses: Vec<&Invocation> = response.responses_for("c1").collect();
    assert_eq!(c1_responses, vec![&inv1, &inv3]);

    // Filter responses for call_id "c2" (expect inv2 only)
    let c2_responses: Vec<&Invocation> = response.responses_for("c2").collect();
    assert_eq!(c2_responses, vec![&inv2]);

    // Filter responses for non-existent call_id (expect empty)
    let unknown_responses: Vec<&Invocation> = response.responses_for("non_existent").collect();
    assert!(unknown_responses.is_empty());
}

#[test]
fn response_wire_serde_roundtrip_and_defaults() {
    let mut created = BTreeMap::new();
    created.insert(Id::from("client_a"), Id::from("server_b"));

    let response = Response::new(State::from("st_42"))
        .with_method_response(Invocation::from_value(
            "Core/echo",
            json!({"status": "ok"}),
            "c0",
        ))
        .with_created_ids(created);

    let serialized = serde_json::to_value(&response).expect("serialize response");
    assert_eq!(serialized["sessionState"], json!("st_42"));
    assert_eq!(serialized["createdIds"]["client_a"], json!("server_b"));
    assert_eq!(
        serialized["methodResponses"][0],
        json!(["Core/echo", {"status": "ok"}, "c0"])
    );

    let restored: Response = serde_json::from_value(serialized).expect("deserialize response");
    assert_eq!(restored, response);

    // Deserialization with missing optional fields falls back to defaults
    let minimal_json = json!({});
    let minimal_response: Response =
        serde_json::from_value(minimal_json).expect("deserialize empty response");
    assert!(minimal_response.method_responses.is_empty());
    assert!(minimal_response.created_ids.is_none());
    assert_eq!(minimal_response.session_state, State::from(""));
}
