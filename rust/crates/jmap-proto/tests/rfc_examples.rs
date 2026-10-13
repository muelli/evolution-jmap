// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC example conformance test suite.
//!
//! Feeds `jmap-proto` the official specifications' OWN examples extracted
//! directly from published RFCs, verifying deserialization, model fidelity,
//! and re-serialization fixed-point stability.

use std::fs;
use std::path::{Path, PathBuf};

use jmap_proto::error::RequestError;
use jmap_proto::methods::{
    ChangesRequest, ChangesResponse, CopyRequest, CopyResponse, GetRequest, SetRequest, SetResponse,
};
use jmap_proto::push::{PushSubscription, PushVerification, StateChange};
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use jmap_proto::session::Session;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn fixture_path(subpath: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rfc")
        .join(subpath)
}

fn load_rfc_fixture<T: DeserializeOwned + Serialize>(subpath: &str) -> (String, T, Value) {
    let path = fixture_path(subpath);
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));

    let mut source_note = String::new();
    let mut json_lines = Vec::new();

    for line in raw.lines() {
        if line.starts_with("//") {
            if source_note.is_empty() {
                source_note = line.trim_start_matches("//").trim().to_string();
            }
        } else {
            json_lines.push(line);
        }
    }

    assert!(
        !source_note.is_empty(),
        "fixture {} must carry a source note on line 1",
        subpath
    );

    let json_text = json_lines.join("\n");
    let input_value: Value = serde_json::from_str(&json_text)
        .unwrap_or_else(|e| panic!("invalid JSON in fixture {}: {e}", subpath));

    let typed: T = serde_json::from_str(&json_text).unwrap_or_else(|e| {
        panic!(
            "failed to deserialize fixture {} into target type: {e}",
            subpath
        )
    });

    (source_note, typed, input_value)
}

fn assert_roundtrip<T>(typed: &T)
where
    T: DeserializeOwned + Serialize + std::fmt::Debug + PartialEq,
{
    let serialized = serde_json::to_string_pretty(typed).expect("serialization should succeed");
    let roundtripped: T = serde_json::from_str(&serialized)
        .expect("deserialization of serialized output should succeed");
    assert_eq!(
        typed, &roundtripped,
        "re-serialization roundtrip must be lossless"
    );
}

// ===========================================================================
// RFC 8620 Core Specifications (July 2019)
// ===========================================================================

#[test]
fn rfc8620_sec_2_1_session() {
    let (note, session, raw) = load_rfc_fixture::<Session>("rfc8620/2.1_session.json");
    assert!(note.contains("RFC 8620 §2.1"));
    assert_roundtrip(&session);

    assert_eq!(session.username, "john@example.com");
    assert_eq!(session.state.as_str(), "75128aab4b1b");
    assert_eq!(session.api_url, "https://jmap.example.com/api/");
    assert_eq!(
        session.download_url,
        "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}"
    );
    assert_eq!(
        session.upload_url,
        "https://jmap.example.com/upload/{accountId}/"
    );
    assert_eq!(
        session.event_source_url,
        "https://jmap.example.com/eventsource/?types={types}&closeafter={closeafter}&ping={ping}"
    );

    let core_cap = session
        .core_capability()
        .expect("core capability must be present");
    assert_eq!(core_cap.max_size_upload, 50_000_000);
    assert_eq!(core_cap.max_concurrent_upload, 8);
    assert_eq!(core_cap.max_size_request, 10_000_000);
    assert_eq!(core_cap.max_concurrent_requests, 8);
    assert_eq!(core_cap.max_calls_in_request, 32);
    assert_eq!(core_cap.max_objects_in_get, 256);
    assert_eq!(core_cap.max_objects_in_set, 128);

    let id_a1 = jmap_proto::id::Id::from("A13824");
    assert!(session.accounts.contains_key(&id_a1));
    let a1 = &session.accounts[&id_a1];
    assert_eq!(a1.name, "john@example.com");
    assert!(a1.is_personal);
    assert!(!a1.is_read_only);

    let id_a2 = jmap_proto::id::Id::from("A97813");
    assert!(session.accounts.contains_key(&id_a2));
    let a2 = &session.accounts[&id_a2];
    assert_eq!(a2.name, "jane@example.com");
    assert!(!a2.is_personal);
    assert!(a2.is_read_only);

    assert_eq!(
        raw["primaryAccounts"]["urn:ietf:params:jmap:mail"],
        "A13824"
    );
    assert_eq!(
        raw["primaryAccounts"]["urn:ietf:params:jmap:contacts"],
        "A13824"
    );
}

#[test]
fn rfc8620_sec_3_3_1_request() {
    let (note, req, _) = load_rfc_fixture::<Request>("rfc8620/3.3.1_request.json");
    assert!(note.contains("RFC 8620 §3.3.1"));
    assert_roundtrip(&req);

    assert_eq!(
        req.using,
        vec!["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"]
    );
    assert_eq!(req.method_calls.len(), 3);
    assert_eq!(req.method_calls[0].name, "method1");
    assert_eq!(req.method_calls[0].call_id, "c1");
    assert_eq!(req.method_calls[0].arguments["arg1"], "arg1data");
    assert_eq!(req.method_calls[1].name, "method2");
    assert_eq!(req.method_calls[1].call_id, "c2");
    assert_eq!(req.method_calls[2].name, "method3");
    assert_eq!(req.method_calls[2].call_id, "c3");
}

#[test]
fn rfc8620_sec_3_4_1_response() {
    let (note, resp, _) = load_rfc_fixture::<Response>("rfc8620/3.4.1_response.json");
    assert!(note.contains("RFC 8620 §3.4.1"));
    assert_roundtrip(&resp);

    assert_eq!(resp.session_state.as_str(), "75128aab4b1b");
    assert_eq!(resp.method_responses.len(), 4);
    assert_eq!(resp.method_responses[0].name, "method1");
    assert_eq!(resp.method_responses[0].call_id, "c1");
    assert_eq!(resp.method_responses[1].name, "method2");
    assert_eq!(resp.method_responses[1].call_id, "c2");
    assert_eq!(resp.method_responses[2].name, "anotherResponseFromMethod2");
    assert_eq!(resp.method_responses[2].call_id, "c2");
    assert_eq!(resp.method_responses[3].name, "error");
    assert_eq!(resp.method_responses[3].call_id, "c3");
    assert_eq!(resp.method_responses[3].arguments["type"], "unknownMethod");
}

#[test]
fn rfc8620_sec_3_6_1_1_unknown_capability() {
    let (note, err, _) =
        load_rfc_fixture::<RequestError>("rfc8620/3.6.1.1_unknown_capability.json");
    assert!(note.contains("RFC 8620 §3.6.1.1"));
    assert_roundtrip(&err);

    assert_eq!(
        err.error_type,
        "urn:ietf:params:jmap:error:unknownCapability"
    );
    assert_eq!(err.status, Some(400));
    assert!(
        err.detail
            .as_deref()
            .unwrap()
            .contains("https://example.com/apis/foobar")
    );
}

#[test]
fn rfc8620_sec_3_6_1_1_limit() {
    let (note, err, _) = load_rfc_fixture::<RequestError>("rfc8620/3.6.1.1_limit.json");
    assert!(note.contains("RFC 8620 §3.6.1.1"));
    assert_roundtrip(&err);

    assert_eq!(err.error_type, "urn:ietf:params:jmap:error:limit");
    assert_eq!(err.status, Some(400));
    assert_eq!(err.extra["limit"], "maxSizeRequest");
}

#[test]
fn rfc8620_sec_3_6_2_method_error() {
    let (note, inv, _) = load_rfc_fixture::<Invocation>("rfc8620/3.6.2_method_error.json");
    assert!(note.contains("RFC 8620 §3.6.2"));
    assert_roundtrip(&inv);

    assert_eq!(inv.name, "error");
    assert_eq!(inv.call_id, "call-id");
    assert_eq!(inv.arguments["type"], "unknownMethod");
}

#[test]
fn rfc8620_sec_3_7_simple_backref_request() {
    let (note, req, _) = load_rfc_fixture::<Request>("rfc8620/3.7_simple_backref_request.json");
    assert!(note.contains("RFC 8620 §3.7"));
    assert_roundtrip(&req);

    assert_eq!(req.using, vec!["urn:ietf:params:jmap:core"]);
    assert_eq!(req.method_calls.len(), 2);
    let backref: ResultReference =
        serde_json::from_value(req.method_calls[1].arguments["#ids"].clone())
            .expect("backref must deserialize into ResultReference");
    assert_eq!(backref.result_of, "t0");
    assert_eq!(backref.name, "Foo/changes");
    assert_eq!(backref.path, "/created");
}

#[test]
fn rfc8620_sec_3_7_thread_backref_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/3.7_thread_backref_request.json");
    assert!(note.contains("RFC 8620 §3.7"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].name, "Email/query");
    assert_eq!(calls[1].name, "Email/get");
    assert_eq!(calls[2].name, "Thread/get");
    assert_eq!(calls[3].name, "Email/get");

    let ref1: ResultReference = serde_json::from_value(calls[1].arguments["#ids"].clone()).unwrap();
    assert_eq!(ref1.result_of, "t0");
    assert_eq!(ref1.path, "/ids");

    let ref2: ResultReference = serde_json::from_value(calls[2].arguments["#ids"].clone()).unwrap();
    assert_eq!(ref2.result_of, "t1");
    assert_eq!(ref2.path, "/list/*/threadId");

    let ref3: ResultReference = serde_json::from_value(calls[3].arguments["#ids"].clone()).unwrap();
    assert_eq!(ref3.result_of, "t2");
    assert_eq!(ref3.path, "/list/*/emailIds");
}

#[test]
fn rfc8620_sec_3_7_thread_backref_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/3.7_thread_backref_response.json");
    assert!(note.contains("RFC 8620 §3.7"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0].name, "Email/query");
    assert_eq!(responses[0].call_id, "t0");
    assert_eq!(responses[1].name, "Email/get");
    assert_eq!(responses[1].call_id, "t1");
    assert_eq!(responses[2].name, "Thread/get");
    assert_eq!(responses[2].call_id, "t2");
}

#[test]
fn rfc8620_sec_3_7_resolved_get_request() {
    let (note, req, _) = load_rfc_fixture::<GetRequest>("rfc8620/3.7_resolved_get_request.json");
    assert!(note.contains("RFC 8620 §3.7"));
    assert_roundtrip(&req);

    assert_eq!(req.account_id.as_str(), "A1");
    let ids = req.ids.as_ref().unwrap();
    assert_eq!(ids.len(), 5);
    assert_eq!(ids[0].as_str(), "msg1020");
    assert_eq!(ids[4].as_str(), "msg223");
    let props = req.properties.as_ref().unwrap();
    assert_eq!(props, &vec!["from", "receivedAt", "subject"]);
}

#[test]
fn rfc8620_sec_4_1_echo_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8620/4.1_echo_request.json");
    assert!(note.contains("RFC 8620 §4.1"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Core/echo");
    assert_eq!(calls[0].call_id, "b3ff");
    assert_eq!(calls[0].arguments["hello"], true);
    assert_eq!(calls[0].arguments["high"], 5);
}

#[test]
fn rfc8620_sec_4_1_echo_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/4.1_echo_response.json");
    assert!(note.contains("RFC 8620 §4.1"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Core/echo");
    assert_eq!(responses[0].call_id, "b3ff");
    assert_eq!(responses[0].arguments["hello"], true);
    assert_eq!(responses[0].arguments["high"], 5);
}

#[test]
fn rfc8620_sec_5_7_query_get_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_query_get_request.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "Todo/query");
    assert_eq!(calls[0].call_id, "0");
    assert_eq!(calls[1].name, "Todo/get");
    assert_eq!(calls[1].call_id, "1");

    let get_req: GetRequest = serde_json::from_value(calls[1].arguments.clone()).unwrap();
    assert_eq!(get_req.account_id.as_str(), "x");
    let ids_ref = get_req.ids_ref.unwrap();
    assert_eq!(ids_ref.result_of, "0");
    assert_eq!(ids_ref.name, "Todo/query");
    assert_eq!(ids_ref.path, "/ids");
}

#[test]
fn rfc8620_sec_5_7_query_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_query_get_response.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Todo/query");
    assert_eq!(responses[1].name, "Todo/get");

    let get_resp: jmap_proto::methods::GetResponse<Value> =
        serde_json::from_value(responses[1].arguments.clone()).unwrap();
    assert_eq!(get_resp.account_id.as_str(), "x");
    assert_eq!(get_resp.state.as_str(), "10324");
    assert_eq!(get_resp.list.len(), 2);
    assert_eq!(get_resp.list[0]["title"], "Practise Piano");
}

#[test]
fn rfc8620_sec_5_7_set_update_full_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_set_update_full_request.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Todo/set");
    let set_req: SetRequest<Value> = serde_json::from_value(calls[0].arguments.clone()).unwrap();
    assert_eq!(set_req.account_id.as_str(), "x");
    assert_eq!(set_req.if_in_state.as_ref().unwrap().as_str(), "10324");
    assert!(
        set_req
            .update
            .as_ref()
            .unwrap()
            .contains_key(&jmap_proto::id::Id::from("a"))
    );
}

#[test]
fn rfc8620_sec_5_7_set_update_patch_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_set_update_patch_request.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Todo/set");
    let set_req: SetRequest<Value> = serde_json::from_value(calls[0].arguments.clone()).unwrap();
    assert_eq!(set_req.account_id.as_str(), "x");
    let patch = &set_req.update.as_ref().unwrap()[&jmap_proto::id::Id::from("a")];
    assert_eq!(patch["keywords/chopin"], true);
    assert_eq!(patch["keywords/mozart"], Value::Null);
}

#[test]
fn rfc8620_sec_5_7_set_update_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_set_update_response.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Todo/set");
    let set_resp: SetResponse<Value> =
        serde_json::from_value(responses[0].arguments.clone()).unwrap();
    assert_eq!(set_resp.account_id.as_str(), "x");
    assert_eq!(set_resp.old_state.as_ref().unwrap().as_str(), "10324");
    assert_eq!(set_resp.new_state.as_ref().unwrap().as_str(), "10329");
    assert!(
        set_resp
            .updated
            .as_ref()
            .unwrap()
            .contains_key(&jmap_proto::id::Id::from("a"))
    );
}

#[test]
fn rfc8620_sec_5_7_set_create_subtask_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_set_create_subtask_request.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    let set_req: SetRequest<Value> = serde_json::from_value(calls[0].arguments.clone()).unwrap();
    assert_eq!(set_req.account_id.as_str(), "x");
    assert!(set_req.create.as_ref().unwrap().contains_key("k15"));
    assert_eq!(
        set_req.update.as_ref().unwrap()[&jmap_proto::id::Id::from("a")]["subTodoIds"][0],
        "#k15"
    );
}

#[test]
fn rfc8620_sec_5_7_changes_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_changes_request.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "Todo/changes");
    let changes_req: ChangesRequest = serde_json::from_value(calls[0].arguments.clone()).unwrap();
    assert_eq!(changes_req.account_id.as_str(), "x");
    assert_eq!(changes_req.since_state.as_str(), "10324");
    assert_eq!(changes_req.max_changes, Some(50));
    assert_eq!(calls[1].name, "Todo/queryChanges");
}

#[test]
fn rfc8620_sec_5_7_changes_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_changes_response.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Todo/changes");
    let changes_resp: ChangesResponse =
        serde_json::from_value(responses[0].arguments.clone()).unwrap();
    assert_eq!(changes_resp.account_id.as_str(), "x");
    assert_eq!(changes_resp.old_state.as_str(), "10324");
    assert_eq!(changes_resp.new_state.as_str(), "871903");
    assert_eq!(changes_resp.destroyed, vec![jmap_proto::id::Id::from("b")]);
    assert!(!changes_resp.has_more_changes);
    assert_eq!(responses[1].name, "Todo/queryChanges");
}

#[test]
fn rfc8620_sec_5_7_copy_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_copy_request.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Todo/copy");
    let copy_req: CopyRequest<Value> = serde_json::from_value(calls[0].arguments.clone()).unwrap();
    assert_eq!(copy_req.from_account_id.as_str(), "x");
    assert_eq!(copy_req.account_id.as_str(), "y");
    assert!(copy_req.on_success_destroy_original);
    assert!(copy_req.create.contains_key("k5122"));
}

#[test]
fn rfc8620_sec_5_7_copy_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/5.7_copy_response.json");
    assert!(note.contains("RFC 8620 §5.7"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Todo/copy");
    assert_eq!(responses[1].name, "Todo/set");

    let copy_resp: CopyResponse<Value> =
        serde_json::from_value(responses[0].arguments.clone()).unwrap();
    assert_eq!(copy_resp.from_account_id.as_str(), "x");
    assert_eq!(copy_resp.account_id.as_str(), "y");
    assert_eq!(
        copy_resp.old_state.as_ref().unwrap().as_str(),
        "c1d64ecb038c"
    );
    assert_eq!(copy_resp.new_state.as_str(), "33844835152b");

    let set_resp: SetResponse<Value> =
        serde_json::from_value(responses[1].arguments.clone()).unwrap();
    assert_eq!(set_resp.account_id.as_str(), "x");
    assert_eq!(set_resp.old_state.as_ref().unwrap().as_str(), "871903");
    assert_eq!(set_resp.new_state.as_ref().unwrap().as_str(), "871909");
    assert_eq!(
        set_resp.destroyed.as_ref().unwrap(),
        &vec![jmap_proto::id::Id::from("a")]
    );
}

#[test]
fn rfc8620_sec_7_1_1_state_change() {
    let (note, sc, _) = load_rfc_fixture::<StateChange>("rfc8620/7.1.1_state_change.json");
    assert!(note.contains("RFC 8620 §7.1.1"));
    assert_roundtrip(&sc);

    assert_eq!(sc.kind, "StateChange");
    assert_eq!(sc.changed.len(), 2);
    let a1 = &sc.changed[&jmap_proto::id::Id::from("a3123")];
    assert_eq!(a1["Email"].as_str(), "d35ecb040aab");
    assert_eq!(a1["EmailDelivery"].as_str(), "428d565f2440");
    assert_eq!(a1["CalendarEvent"].as_str(), "87accfac587a");
    let a2 = &sc.changed[&jmap_proto::id::Id::from("a43461d")];
    assert_eq!(a2["Mailbox"].as_str(), "0af7a512ce70");
    assert_eq!(a2["CalendarEvent"].as_str(), "7a4297cecd76");
}

#[test]
fn rfc8620_sec_7_2_3_push_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8620/7.2.3_get_request.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "PushSubscription/get");
    assert_eq!(calls[0].arguments["ids"], Value::Null);
}

#[test]
fn rfc8620_sec_7_2_3_push_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/7.2.3_get_response.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "PushSubscription/get");
    let subs: Vec<PushSubscription> =
        serde_json::from_value(responses[0].arguments["list"].clone()).unwrap();
    assert_eq!(subs.len(), 2);
    assert_eq!(
        subs[0].id.as_ref().unwrap().as_str(),
        "e50b2c1d-9553-41a3-b0a7-a7d26b599ee1"
    );
    assert_eq!(subs[0].device_client_id, "b37ff8001ca0");
    assert_eq!(subs[0].types.as_ref().unwrap(), &vec!["Todo"]);
    assert_eq!(subs[1].device_client_id, "X8980fc");
    assert_eq!(
        subs[1].types.as_ref().unwrap(),
        &vec!["Mailbox", "Email", "EmailDelivery"]
    );
}

#[test]
fn rfc8620_sec_7_2_3_push_set_create_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/7.2.3_set_create_request.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "PushSubscription/set");
    let create_map = &calls[0].arguments["create"];
    assert_eq!(create_map["4f29"]["deviceClientId"], "a889-ffea-910");
}

#[test]
fn rfc8620_sec_7_2_3_push_set_created_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/7.2.3_set_created_response.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "PushSubscription/set");
    let created = &responses[0].arguments["created"]["4f29"];
    assert_eq!(created["id"], "P43dcfa4-1dd4-41ef-9156-2c89b3b19c60");
}

#[test]
fn rfc8620_sec_7_2_3_push_verification() {
    let (note, pv, _) = load_rfc_fixture::<PushVerification>("rfc8620/7.2.3_verification.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&pv);

    assert_eq!(pv.object_type, "PushVerification");
    assert_eq!(
        pv.push_subscription_id.as_str(),
        "P43dcfa4-1dd4-41ef-9156-2c89b3b19c60"
    );
    assert_eq!(pv.verification_code, "da1f097b11ca17f06424e30bf02bfa67");
}

#[test]
fn rfc8620_sec_7_2_3_push_set_update_code_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/7.2.3_set_update_code_request.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "PushSubscription/set");
    let update = &calls[0].arguments["update"]["P43dcfa4-1dd4-41ef-9156-2c89b3b19c60"];
    assert_eq!(
        update["verificationCode"],
        "da1f097b11ca17f06424e30bf02bfa67"
    );
}

#[test]
fn rfc8620_sec_7_2_3_push_set_update_expires_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/7.2.3_set_update_expires_request.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "PushSubscription/set");
    let update = &calls[0].arguments["update"]["P43dcfa4-1dd4-41ef-9156-2c89b3b19c60"];
    assert_eq!(update["expires"], "2018-08-13T00:00:00Z");
}

#[test]
fn rfc8620_sec_7_2_3_push_set_update_expires_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8620/7.2.3_set_update_expires_response.json");
    assert!(note.contains("RFC 8620 §7.2.3"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "PushSubscription/set");
    let updated = &responses[0].arguments["updated"]["P43dcfa4-1dd4-41ef-9156-2c89b3b19c60"];
    assert_eq!(updated["expires"], "2018-07-15T02:22:50Z");
}
