// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC example conformance test suite.
//!
//! Feeds `jmap-proto` the official specifications' OWN examples extracted
//! directly from published RFCs, verifying deserialization, model fidelity,
//! and re-serialization fixed-point stability.

use std::fs;
use std::path::{Path, PathBuf};

use jmap_proto::blob::{
    BlobGetRequest, BlobGetResponse, BlobLookupRequest, BlobLookupResponse, BlobUploadRequest,
    BlobUploadResponse,
};
use jmap_proto::calendars::{
    Calendar, CalendarAlert, CalendarEvent, CalendarEventParseRequest, CalendarEventSetRequest,
    CalendarGroup, CalendarSetRequest,
};
use jmap_proto::contacts::{AddressBook, AddressBookSetRequest, ContactCard, Name};
use jmap_proto::error::RequestError;
use jmap_proto::id::Id;
use jmap_proto::mail::{
    Email, EmailAddress, EmailAddressGroup, EmailGetRequest, EmailSubmission,
    EmailSubmissionSetRequest, Identity, Mailbox, SearchSnippetGetRequest,
    SearchSnippetGetResponse, Thread,
};
use jmap_proto::methods::{
    ChangesRequest, ChangesResponse, CopyRequest, CopyResponse, GetRequest, GetResponse,
    SetRequest, SetResponse,
};
use jmap_proto::principals::Principal;
use jmap_proto::push::{PushSubscription, PushVerification, StateChange};
use jmap_proto::quota::Quota;
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use jmap_proto::session::Session;
use jmap_proto::sieve::{SieveCapability, SieveScript, SieveScriptSetRequest};
use jmap_proto::tasks::Task;
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

// ===========================================================================
// RFC 8621 Mail Specifications (August 2019)
// ===========================================================================

#[test]
fn rfc8621_sec_2_6_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/2.6_get_request.json");
    assert!(note.contains("RFC 8621 §2.6"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Mailbox/get");
    let get_req: GetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(get_req.account_id.as_str(), "u33084183");
    assert!(get_req.ids.is_none());
}

#[test]
fn rfc8621_sec_2_6_get_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/2.6_get_response.json");
    assert!(note.contains("RFC 8621 §2.6"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Mailbox/get");
    let res: GetResponse<Mailbox> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<Mailbox>");
    assert_eq!(res.account_id.as_str(), "u33084183");
    assert_eq!(res.state.as_str(), "78540");
    assert_eq!(res.list.len(), 2);

    let inbox = &res.list[0];
    assert_eq!(inbox.id, Some(Id::from("MB23cfa8094c0f41e6")));
    assert_eq!(inbox.name, "Inbox");
    assert_eq!(inbox.role.as_deref(), Some("inbox"));
    assert_eq!(inbox.sort_order, Some(10));
    assert_eq!(inbox.total_emails, Some(16307));
    assert_eq!(inbox.unread_emails, Some(13905));
    assert_eq!(inbox.total_threads, Some(5833));
    assert_eq!(inbox.unread_threads, Some(5128));
    assert!(inbox.is_subscribed.unwrap());
    assert_eq!(inbox.my_rights.as_ref().unwrap().may_read_items, Some(true));

    assert_eq!(res.list[1].name, "Important mail");
}

#[test]
fn rfc8621_sec_2_6_changes_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/2.6_changes_request.json");
    assert!(note.contains("RFC 8621 §2.6"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].name, "Mailbox/changes");
    assert_eq!(calls[1].name, "Mailbox/get");
    assert_eq!(calls[2].name, "Mailbox/get");

    let changes: ChangesRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into ChangesRequest");
    assert_eq!(changes.account_id.as_str(), "u33084183");
    assert_eq!(changes.since_state.as_str(), "78540");
}

#[test]
fn rfc8621_sec_2_6_changes_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/2.6_changes_response.json");
    assert!(note.contains("RFC 8621 §2.6"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0].name, "Mailbox/changes");
    assert_eq!(responses[1].name, "Mailbox/get");
    assert_eq!(responses[2].name, "Mailbox/get");

    let changes: ChangesResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into ChangesResponse");
    assert_eq!(changes.old_state.as_str(), "78541");
    assert_eq!(changes.new_state.as_str(), "78542");
    assert!(!changes.has_more_changes);
    assert_eq!(changes.updated, vec![Id::from("MB23cfa8094c0f41e6")]);

    let get_res: GetResponse<Mailbox> = serde_json::from_value(responses[2].arguments.clone())
        .expect("arguments must deserialize into GetResponse<Mailbox>");
    assert_eq!(get_res.list.len(), 1);
    assert_eq!(get_res.list[0].total_emails, Some(16307));
    assert_eq!(get_res.list[0].unread_emails, Some(13903));
}

#[test]
fn rfc8621_sec_2_6_set_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/2.6_set_request.json");
    assert!(note.contains("RFC 8621 §2.6"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Mailbox/set");
    let set: SetRequest<Mailbox> = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SetRequest<Mailbox>");
    assert_eq!(set.account_id.as_str(), "u33084183");
    assert_eq!(set.if_in_state.unwrap().as_str(), "78542");
    assert!(
        set.update
            .as_ref()
            .unwrap()
            .contains_key(&Id::from("MB674cc24095db49ce"))
    );
    assert_eq!(set.destroy, Some(vec![Id::from("MB23cfa8094c0f41e6")]));
}

#[test]
fn rfc8621_sec_2_6_set_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/2.6_set_response.json");
    assert!(note.contains("RFC 8621 §2.6"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Mailbox/set");
    let res: SetResponse<Mailbox> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<Mailbox>");
    assert_eq!(res.old_state.as_ref().unwrap().as_str(), "78542");
    assert_eq!(res.new_state.as_ref().unwrap().as_str(), "78549");
    assert!(
        res.updated
            .as_ref()
            .unwrap()
            .contains_key(&Id::from("MB674cc24095db49ce"))
    );
    assert_eq!(
        res.not_destroyed.as_ref().unwrap()[&Id::from("MB23cfa8094c0f41e6")].error_type,
        "forbidden"
    );
}

#[test]
fn rfc8621_sec_3_1_1_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/3.1.1_get_request.json");
    assert!(note.contains("RFC 8621 §3.1.1"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Thread/get");
    let get_req: GetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(get_req.account_id.as_str(), "acme");
    assert_eq!(
        get_req.ids,
        Some(vec![Id::from("f123u4"), Id::from("f41u44")])
    );
}

#[test]
fn rfc8621_sec_3_1_1_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/3.1.1_get_response.json");
    assert!(note.contains("RFC 8621 §3.1.1"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Thread/get");
    let res: GetResponse<Thread> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<Thread>");
    assert_eq!(res.account_id.as_str(), "acme");
    assert_eq!(res.state.as_str(), "f6a7e214");
    assert_eq!(res.list.len(), 2);
    assert_eq!(res.list[0].id, Some(Id::from("f123u4")));
    assert_eq!(
        res.list[0].email_ids,
        vec![Id::from("eaa623"), Id::from("f782cbb")]
    );
    assert_eq!(res.list[1].id, Some(Id::from("f41u44")));
    assert_eq!(res.list[1].email_ids, vec![Id::from("82cf7bb")]);
}

#[test]
fn rfc8621_sec_4_1_2_3_addresses() {
    let (note, addrs, _) = load_rfc_fixture::<Vec<EmailAddress>>("rfc8621/4.1.2.3_addresses.json");
    assert!(note.contains("RFC 8621 §4.1.2.3"));
    assert_roundtrip(&addrs);

    assert_eq!(addrs.len(), 3);
    assert_eq!(addrs[0].name.as_deref(), Some("James Smythe"));
    assert_eq!(addrs[0].email, "james@example.com");
    assert!(addrs[1].name.is_none());
    assert_eq!(addrs[1].email, "jane@example.com");
    assert_eq!(addrs[2].name.as_deref(), Some("John Smith"));
    assert_eq!(addrs[2].email, "john@example.com");
}

#[test]
fn rfc8621_sec_4_1_2_4_grouped_addresses() {
    let (note, groups, _) =
        load_rfc_fixture::<Vec<EmailAddressGroup>>("rfc8621/4.1.2.4_grouped_addresses.json");
    assert!(note.contains("RFC 8621 §4.1.2.4"));
    assert_roundtrip(&groups);

    assert_eq!(groups.len(), 2);
    assert!(groups[0].name.is_none());
    assert_eq!(groups[0].addresses.len(), 1);
    assert_eq!(groups[0].addresses[0].email, "james@example.com");
    assert_eq!(groups[1].name.as_deref(), Some("Friends"));
    assert_eq!(groups[1].addresses.len(), 2);
    assert_eq!(groups[1].addresses[0].email, "jane@example.com");
    assert_eq!(groups[1].addresses[1].email, "john@example.com");
}

#[test]
fn rfc8621_sec_4_2_1_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.2.1_get_request.json");
    assert!(note.contains("RFC 8621 §4.2.1"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Email/get");
    let req: EmailGetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into EmailGetRequest");
    assert_eq!(
        req.ids,
        Some(vec![Id::from("f123u456"), Id::from("f123u457")])
    );
    assert!(req.fetch_html_body_values);
    assert_eq!(req.max_body_value_bytes, Some(256));
    assert_eq!(
        req.body_properties,
        Some(vec![
            "partId".to_string(),
            "blobId".to_string(),
            "size".to_string(),
            "type".to_string()
        ])
    );
}

#[test]
fn rfc8621_sec_4_2_1_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.2.1_get_response.json");
    assert!(note.contains("RFC 8621 §4.2.1"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Email/get");
    let res: GetResponse<Email> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<Email>");
    assert_eq!(res.account_id.as_str(), "abc");
    assert_eq!(res.state.as_str(), "41234123231");
    assert_eq!(res.list.len(), 1);

    let em = &res.list[0];
    assert_eq!(em.id, Some(Id::from("f123u457")));
    assert_eq!(em.thread_id, Some(Id::from("ef1314a")));
    assert_eq!(em.subject.as_deref(), Some("Dinner on Thursday?"));
    assert_eq!(em.html_body.as_ref().unwrap().len(), 2);
    assert_eq!(
        em.html_body.as_ref().unwrap()[0].part_id.as_deref(),
        Some("1")
    );
    assert_eq!(
        em.body_values.as_ref().unwrap()["1"].value,
        "<html><body><p>Hello ..."
    );
    assert!(em.body_values.as_ref().unwrap()["1"].is_truncated);
    assert_eq!(res.not_found, vec![Id::from("f123u456")]);
}

#[test]
fn rfc8621_sec_4_10_initial_sync_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_initial_sync_request.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].name, "Email/query");
    assert_eq!(calls[1].name, "Email/get");
    assert_eq!(calls[2].name, "Thread/get");
    assert_eq!(calls[3].name, "Email/get");
}

#[test]
fn rfc8621_sec_4_10_initial_sync_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_initial_sync_response.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 4);
    assert_eq!(responses[0].name, "Email/query");
    assert_eq!(responses[1].name, "Email/get");
    assert_eq!(responses[2].name, "Thread/get");
    assert_eq!(responses[3].name, "Email/get");

    let em_get: GetResponse<Email> = serde_json::from_value(responses[3].arguments.clone())
        .expect("arguments must deserialize into GetResponse<Email>");
    assert_eq!(em_get.list.len(), 1);
    assert_eq!(em_get.list[0].subject.as_deref(), Some("The Big Reveal"));
    assert!(em_get.list[0].has_attachment.unwrap());
}

#[test]
fn rfc8621_sec_4_10_set_update_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_set_update_request.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Email/set");
    let set: SetRequest<Email> = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SetRequest<Email>");
    assert_eq!(set.account_id.as_str(), "ue150411c");
    assert!(
        set.update
            .unwrap()
            .contains_key(&Id::from("Ma783e5cdf5f2deffbc97930a"))
    );
}

#[test]
fn rfc8621_sec_4_10_set_update_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_set_update_response.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Email/set");
    let res: SetResponse<Email> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<Email>");
    assert_eq!(res.old_state.as_ref().unwrap().as_str(), "780605");
    assert_eq!(res.new_state.as_ref().unwrap().as_str(), "780606");
    assert!(
        res.updated
            .unwrap()
            .contains_key(&Id::from("Ma783e5cdf5f2deffbc97930a"))
    );
}

#[test]
fn rfc8621_sec_4_10_resync_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_resync_request.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "Email/changes");
    assert_eq!(calls[1].name, "Email/queryChanges");
}

#[test]
fn rfc8621_sec_4_10_resync_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_resync_response.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Email/changes");
    assert_eq!(responses[1].name, "Email/queryChanges");

    let changes: ChangesResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into ChangesResponse");
    assert_eq!(changes.created, vec![Id::from("Me8de6c9f6de198239b982ea2")]);
    assert_eq!(changes.updated, vec![Id::from("Ma783e5cdf5f2deffbc97930a")]);
    assert_eq!(
        changes.destroyed,
        vec![Id::from("M9bd17497e2a99cb345fc1d0a")]
    );
}

#[test]
fn rfc8621_sec_4_10_set_create_text_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_set_create_text_request.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Email/set");
    let set: SetRequest<Email> = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SetRequest<Email>");
    let draft = &set.create.unwrap()["k192"];
    assert_eq!(draft.subject.as_deref(), Some("World domination"));
    assert_eq!(
        draft
            .body_structure
            .as_ref()
            .unwrap()
            .content_type
            .as_deref(),
        Some("text/plain")
    );
}

#[test]
fn rfc8621_sec_4_10_set_create_text_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_set_create_text_response.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Email/set");
    let res: SetResponse<Email> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<Email>");
    let created = &res.created.unwrap()["k192"];
    assert_eq!(
        created.id.as_ref().map(|id| id.as_str()),
        Some("Mf40b5f831efa7233b9eb1c7f")
    );
    assert_eq!(created.size, Some(359));
}

#[test]
fn rfc8621_sec_4_10_set_create_html_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_set_create_html_request.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Email/set");
    let set: SetRequest<Email> = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SetRequest<Email>");
    let draft = &set.create.unwrap()["k1546"];
    assert_eq!(draft.subject.as_deref(), Some("World domination"));
    let bs = draft.body_structure.as_ref().unwrap();
    assert_eq!(bs.content_type.as_deref(), Some("multipart/alternative"));
    assert_eq!(bs.sub_parts.as_ref().unwrap().len(), 2);
    assert_eq!(
        set.destroy,
        Some(vec![Id::from("Mf40b5f831efa7233b9eb1c7f")])
    );
}

#[test]
fn rfc8621_sec_4_10_set_create_html_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_set_create_html_response.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Email/set");
    let res: SetResponse<Email> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<Email>");
    assert_eq!(
        res.destroyed,
        Some(vec![Id::from("Mf40b5f831efa7233b9eb1c7f")])
    );
}

#[test]
fn rfc8621_sec_4_10_copy_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_copy_request.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Email/copy");
    let copy: CopyRequest<Email> = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into CopyRequest<Email>");
    assert_eq!(copy.from_account_id.as_str(), "ue150411c");
    assert_eq!(copy.account_id.as_str(), "u6c6c41ac");
    assert!(copy.on_success_destroy_original);
}

#[test]
fn rfc8621_sec_4_10_copy_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/4.10_copy_response.json");
    assert!(note.contains("RFC 8621 §4.10"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Email/copy");
    assert_eq!(responses[1].name, "Email/set");

    let copy_res: CopyResponse<Email> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into CopyResponse<Email>");
    assert_eq!(
        copy_res.old_state.as_ref().unwrap().as_str(),
        "7ee7e9263a6d"
    );
    assert_eq!(copy_res.new_state.as_str(), "5a0d2447ed26");

    let set_res: SetResponse<Email> = serde_json::from_value(responses[1].arguments.clone())
        .expect("arguments must deserialize into SetResponse<Email>");
    assert_eq!(
        set_res.destroyed,
        Some(vec![Id::from("Md45b47b4877521042cec0938")])
    );
}

#[test]
fn rfc8621_sec_5_2_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/5.2_get_request.json");
    assert!(note.contains("RFC 8621 §5.2"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "SearchSnippet/get");
    let req: SearchSnippetGetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SearchSnippetGetRequest");
    assert_eq!(req.account_id.as_str(), "ue150411c");
    assert_eq!(req.email_ids.len(), 3);
}

#[test]
fn rfc8621_sec_5_2_get_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>("rfc8621/5.2_get_response.json");
    assert!(note.contains("RFC 8621 §5.2"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "SearchSnippet/get");
    let res: SearchSnippetGetResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SearchSnippetGetResponse");
    assert_eq!(res.account_id.as_str(), "ue150411c");
    assert_eq!(res.list.len(), 3);
    assert!(
        res.list[1]
            .subject
            .as_ref()
            .unwrap()
            .contains("The <mark>Foo</mark>sball")
    );
}

#[test]
fn rfc8621_sec_6_4_get_request() {
    let (note, inv, _) = load_rfc_fixture::<Invocation>("rfc8621/6.4_get_request.json");
    assert!(note.contains("RFC 8621 §6.4"));
    assert_roundtrip(&inv);

    assert_eq!(inv.name, "Identity/get");
    let req: GetRequest =
        serde_json::from_value(inv.arguments).expect("arguments must deserialize into GetRequest");
    assert_eq!(req.account_id.as_str(), "acme");
}

#[test]
fn rfc8621_sec_6_4_get_response() {
    let (note, inv, _) = load_rfc_fixture::<Invocation>("rfc8621/6.4_get_response.json");
    assert!(note.contains("RFC 8621 §6.4"));
    assert_roundtrip(&inv);

    assert_eq!(inv.name, "Identity/get");
    let res: GetResponse<Identity> = serde_json::from_value(inv.arguments)
        .expect("arguments must deserialize into GetResponse<Identity>");
    assert_eq!(res.account_id.as_str(), "acme");
    assert_eq!(res.list.len(), 2);
    assert_eq!(res.list[0].name, "Joe Bloggs");
    assert_eq!(res.list[0].email, "joe@example.com");
}

#[test]
fn rfc8621_sec_7_5_1_set_create_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/7.5.1_set_create_request.json");
    assert!(note.contains("RFC 8621 §7.5.1"));
    assert_roundtrip(&calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "EmailSubmission/set");
    let req: EmailSubmissionSetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into EmailSubmissionSetRequest");
    assert_eq!(req.set.account_id.as_str(), "ue411d190");
    assert!(req.set.create.as_ref().unwrap().contains_key("k1490"));
    assert!(req.on_success_update_email.is_some());
}

#[test]
fn rfc8621_sec_7_5_1_set_created_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/7.5.1_set_created_response.json");
    assert!(note.contains("RFC 8621 §7.5.1"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "EmailSubmission/set");
    assert_eq!(responses[1].name, "Email/set");

    let sub_res: SetResponse<EmailSubmission> =
        serde_json::from_value(responses[0].arguments.clone())
            .expect("arguments must deserialize into SetResponse<EmailSubmission>");
    assert_eq!(
        sub_res.created.unwrap()["k1490"]
            .id
            .as_ref()
            .map(|id| id.as_str()),
        Some("ES-3bab7f9a-623e-4acf-99a5-2e67facb02a0")
    );

    let em_res: SetResponse<Email> = serde_json::from_value(responses[1].arguments.clone())
        .expect("arguments must deserialize into SetResponse<Email>");
    assert!(
        em_res
            .updated
            .unwrap()
            .contains_key(&Id::from("M7f6ed5bcfd7e2604d1753f6c"))
    );
}

#[test]
fn rfc8621_sec_7_5_1_set_error_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc8621/7.5.1_set_error_response.json");
    assert!(note.contains("RFC 8621 §7.5.1"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "EmailSubmission/set");
    let res: SetResponse<EmailSubmission> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<EmailSubmission>");
    let err = &res.not_created.unwrap()["k1490"];
    assert_eq!(err.error_type, "forbiddenToSend");
    assert!(err.description.as_ref().unwrap().contains("Verzeihung"));
}

// ===========================================================================
// RFC 9610 Contacts (December 2024)
// ===========================================================================

#[test]
fn rfc9610_sec_4_1_get_request() {
    let (note, invocations, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9610/4.1_get_request.json");
    assert!(note.contains("RFC 9610 §4.1"));
    assert_roundtrip(&invocations);

    assert_eq!(invocations.len(), 2);
    assert_eq!(invocations[0].name, "AddressBook/get");
    assert_eq!(invocations[0].call_id, "0");
    let ab_get: GetRequest = serde_json::from_value(invocations[0].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(ab_get.account_id.as_str(), "a0x9");

    assert_eq!(invocations[1].name, "ContactCard/get");
    assert_eq!(invocations[1].call_id, "1");
    let cc_get: GetRequest = serde_json::from_value(invocations[1].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(cc_get.account_id.as_str(), "a0x9");
}

#[test]
fn rfc9610_sec_4_1_get_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9610/4.1_get_response.json");
    assert!(note.contains("RFC 9610 §4.1"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "AddressBook/get");
    let ab_res: GetResponse<AddressBook> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<AddressBook>");
    assert_eq!(ab_res.list.len(), 2);
    assert_eq!(ab_res.list[0].name, "Personal");
    assert_eq!(ab_res.list[0].is_default, Some(true));
    assert_eq!(ab_res.list[1].name, "Autosaved");
    assert_eq!(ab_res.list[1].is_default, Some(false));

    assert_eq!(responses[1].name, "ContactCard/get");
    let cc_res: GetResponse<ContactCard> = serde_json::from_value(responses[1].arguments.clone())
        .expect("arguments must deserialize into GetResponse<ContactCard>");
    assert_eq!(cc_res.list.len(), 1);
    let card = &cc_res.list[0];
    assert_eq!(card.id.as_ref().unwrap().as_str(), "3");
    assert!(
        card.address_book_ids
            .as_ref()
            .unwrap()
            .contains_key(&Id::from("062adcfa-105d-455c-bc60-6db68b69c3f3"))
    );
    assert_eq!(
        card.name
            .as_ref()
            .unwrap()
            .components
            .as_ref()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn rfc9610_sec_4_2_set_request() {
    let (note, invocations, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9610/4.2_set_request.json");
    assert!(note.contains("RFC 9610 §4.2"));
    assert_roundtrip(&invocations);

    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].name, "AddressBook/set");
    let set_req: AddressBookSetRequest = serde_json::from_value(invocations[0].arguments.clone())
        .expect("arguments must deserialize into AddressBookSetRequest");
    assert_eq!(set_req.set.account_id.as_str(), "a0x9");
    assert_eq!(
        set_req.on_success_set_is_default.as_ref().unwrap().as_str(),
        "cd40089d-35f9-4fd7-980b-ba3a9f1d74fe"
    );
}

#[test]
fn rfc9610_sec_4_2_set_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9610/4.2_set_response.json");
    assert!(note.contains("RFC 9610 §4.2"));
    assert_roundtrip(&responses);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "AddressBook/set");
    // Note: In RFC 9610 §4.2 Figure 4, oldState and newState were mistakenly
    // placed inside the 'updated' map rather than at the top level of SetResponse.
    assert_eq!(responses[0].arguments["accountId"], "a0x9");
    assert!(responses[0].arguments["updated"].is_object());
    assert_eq!(
        responses[0].arguments["updated"]["cd40089d-35f9-4fd7-980b-ba3a9f1d74fe"]["isDefault"],
        true
    );
}

// ===========================================================================
// RFC 9553 JSContact (May 2024)
// ===========================================================================

#[test]
fn rfc9553_sec_1_4_5_phonetic() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/1.4.5_phonetic.json");
    assert!(note.contains("RFC 9553 §1.4.5"));
    assert_roundtrip(&card);
    assert!(card.name.is_some());
}

#[test]
fn rfc9553_sec_1_6_2_vendor_properties() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/1.6.2_vendor_properties.json");
    assert!(note.contains("RFC 9553 §1.6.2"));
    assert_roundtrip(&card);
    assert!(card.extra.contains_key("example.com:foo"));
}

#[test]
fn rfc9553_sec_1_6_2_vendor_value() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/1.6.2_vendor_value.json");
    assert!(note.contains("RFC 9553 §1.6.2"));
    assert_roundtrip(&card);
    assert_eq!(card.kind.as_deref(), Some("example.com:baz"));
}

#[test]
fn rfc9553_sec_2_1_basic_card() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1_basic_card.json");
    assert!(note.contains("RFC 9553 §2.1"));
    assert_roundtrip(&card);
    assert_eq!(card.version.as_deref(), Some("1.0"));
    assert_eq!(
        card.name
            .as_ref()
            .unwrap()
            .components
            .as_ref()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn rfc9553_sec_2_1_2_version() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.2_version.json");
    assert!(note.contains("RFC 9553 §2.1.2"));
    assert_roundtrip(&card);
    assert_eq!(card.version.as_deref(), Some("1.0"));
}

#[test]
fn rfc9553_sec_2_1_3_created() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.3_created.json");
    assert!(note.contains("RFC 9553 §2.1.3"));
    assert_roundtrip(&card);
    assert!(card.created.is_some());
}

#[test]
fn rfc9553_sec_2_1_4_kind() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.4_kind.json");
    assert!(note.contains("RFC 9553 §2.1.4"));
    assert_roundtrip(&card);
    assert_eq!(card.kind.as_deref(), Some("individual"));
}

#[test]
fn rfc9553_sec_2_1_5_language() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.5_language.json");
    assert!(note.contains("RFC 9553 §2.1.5"));
    assert_roundtrip(&card);
    assert_eq!(card.language.as_deref(), Some("de-AT"));
}

#[test]
fn rfc9553_sec_2_1_6_members() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.6_members.json");
    assert!(note.contains("RFC 9553 §2.1.6"));
    assert_roundtrip(&card);
    assert_eq!(card.kind.as_deref(), Some("group"));
    assert_eq!(card.members.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_1_7_prod_id() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.7_prod_id.json");
    assert!(note.contains("RFC 9553 §2.1.7"));
    assert_roundtrip(&card);
    assert!(card.prod_id.as_ref().unwrap().contains("ACME Contacts App"));
}

#[test]
fn rfc9553_sec_2_1_8_related_to() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.8_related_to.json");
    assert!(note.contains("RFC 9553 §2.1.8"));
    assert_roundtrip(&card);
    assert_eq!(card.related_to.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_1_9_uid() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.9_uid.json");
    assert!(note.contains("RFC 9553 §2.1.9"));
    assert_roundtrip(&card);
    assert_eq!(
        card.uid.as_deref(),
        Some("urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6")
    );
}

#[test]
fn rfc9553_sec_2_1_10_updated() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.1.10_updated.json");
    assert!(note.contains("RFC 9553 §2.1.10"));
    assert_roundtrip(&card);
    assert!(card.updated.is_some());
}

#[test]
fn rfc9553_sec_2_2_1_name_two_words() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.2.1_name_two_words.json");
    assert!(note.contains("RFC 9553 §2.2.1"));
    assert_roundtrip(&card);
    assert_eq!(
        card.name
            .as_ref()
            .unwrap()
            .components
            .as_ref()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn rfc9553_sec_2_2_1_name_second_surname() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.2.1_name_second_surname.json");
    assert!(note.contains("RFC 9553 §2.2.1"));
    assert_roundtrip(&card);
    assert_eq!(
        card.name
            .as_ref()
            .unwrap()
            .components
            .as_ref()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn rfc9553_sec_2_2_1_name_full() {
    let (note, card, _) = load_rfc_fixture::<Name>("rfc9553/2.2.1_name_full.json");
    assert!(note.contains("RFC 9553 §2.2.1"));
    assert_roundtrip(&card);
    assert_eq!(card.full.as_deref(), Some("Mr. John Q. Public, Esq."));
}

#[test]
fn rfc9553_sec_2_2_1_name_sort_as() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.2.1_name_sort_as.json");
    assert!(note.contains("RFC 9553 §2.2.1"));
    assert_roundtrip(&card);
    assert!(card.name.as_ref().unwrap().sort_as.is_some());
}

#[test]
fn rfc9553_sec_2_2_1_name_phonetic_localizations() {
    let (note, card, _) =
        load_rfc_fixture::<ContactCard>("rfc9553/2.2.1_name_phonetic_localizations.json");
    assert!(note.contains("RFC 9553 §2.2.1"));
    assert_roundtrip(&card);
    assert!(card.localizations.is_some());
    assert_eq!(card.language.as_deref(), Some("zh-Hant"));
}

#[test]
fn rfc9553_sec_2_2_2_nicknames() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.2.2_nicknames.json");
    assert!(note.contains("RFC 9553 §2.2.2"));
    assert_roundtrip(&card);
    assert!(card.nicknames.as_ref().unwrap().contains_key("k391"));
}

#[test]
fn rfc9553_sec_2_2_3_organizations() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.2.3_organizations.json");
    assert!(note.contains("RFC 9553 §2.2.3"));
    assert_roundtrip(&card);
    assert_eq!(card.organizations.as_ref().unwrap().len(), 1);
}

#[test]
fn rfc9553_sec_2_2_4_speak_to_as() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.2.4_speak_to_as.json");
    assert!(note.contains("RFC 9553 §2.2.4"));
    assert_roundtrip(&card);
    assert_eq!(
        card.speak_to_as
            .as_ref()
            .unwrap()
            .grammatical_gender
            .as_deref(),
        Some("neuter")
    );
    assert!(card.speak_to_as.as_ref().unwrap().pronouns.is_some());
}

#[test]
fn rfc9553_sec_2_2_5_titles() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.2.5_titles.json");
    assert!(note.contains("RFC 9553 §2.2.5"));
    assert_roundtrip(&card);
    assert_eq!(card.titles.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_3_1_emails() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.3.1_emails.json");
    assert!(note.contains("RFC 9553 §2.3.1"));
    assert_roundtrip(&card);
    assert_eq!(card.emails.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_3_2_online_services() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.3.2_online_services.json");
    assert!(note.contains("RFC 9553 §2.3.2"));
    assert_roundtrip(&card);
    assert_eq!(card.online_services.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_3_3_phones() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.3.3_phones.json");
    assert!(note.contains("RFC 9553 §2.3.3"));
    assert_roundtrip(&card);
    assert_eq!(card.phones.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_3_4_preferred_languages() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.3.4_preferred_languages.json");
    assert!(note.contains("RFC 9553 §2.3.4"));
    assert_roundtrip(&card);
    assert_eq!(card.preferred_languages.as_ref().unwrap().len(), 3);
}

#[test]
fn rfc9553_sec_2_4_1_calendars() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.4.1_calendars.json");
    assert!(note.contains("RFC 9553 §2.4.1"));
    assert_roundtrip(&card);
    assert_eq!(card.calendars.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_4_2_scheduling_addresses() {
    let (note, card, _) =
        load_rfc_fixture::<ContactCard>("rfc9553/2.4.2_scheduling_addresses.json");
    assert!(note.contains("RFC 9553 §2.4.2"));
    assert_roundtrip(&card);
    assert!(
        card.scheduling_addresses
            .as_ref()
            .unwrap()
            .contains_key("sched1")
    );
}

#[test]
fn rfc9553_sec_2_5_1_address_usa() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.5.1_address_usa.json");
    assert!(note.contains("RFC 9553 §2.5.1"));
    assert_roundtrip(&card);
    assert!(card.addresses.as_ref().unwrap().contains_key("k23"));
}

#[test]
fn rfc9553_sec_2_5_1_address_thailand() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.5.1_address_thailand.json");
    assert!(note.contains("RFC 9553 §2.5.1"));
    assert_roundtrip(&card);
    assert!(card.addresses.as_ref().unwrap().contains_key("k25"));
}

#[test]
fn rfc9553_sec_2_5_1_address_tokyo() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.5.1_address_tokyo.json");
    assert!(note.contains("RFC 9553 §2.5.1"));
    assert_roundtrip(&card);
    assert!(card.addresses.as_ref().unwrap().contains_key("k26"));
    assert!(card.localizations.is_some());
}

#[test]
fn rfc9553_sec_2_6_1_crypto_keys_external() {
    let (note, card, _) =
        load_rfc_fixture::<ContactCard>("rfc9553/2.6.1_crypto_keys_external.json");
    assert!(note.contains("RFC 9553 §2.6.1"));
    assert_roundtrip(&card);
    assert!(card.crypto_keys.as_ref().unwrap().contains_key("mykey1"));
}

#[test]
fn rfc9553_sec_2_6_1_crypto_keys_embedded() {
    let (note, card, _) =
        load_rfc_fixture::<ContactCard>("rfc9553/2.6.1_crypto_keys_embedded.json");
    assert!(note.contains("RFC 9553 §2.6.1"));
    assert_roundtrip(&card);
    assert!(card.crypto_keys.as_ref().unwrap().contains_key("mykey2"));
}

#[test]
fn rfc9553_sec_2_6_2_directories() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.6.2_directories.json");
    assert!(note.contains("RFC 9553 §2.6.2"));
    assert_roundtrip(&card);
    assert_eq!(card.directories.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_6_3_links() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.6.3_links.json");
    assert!(note.contains("RFC 9553 §2.6.3"));
    assert_roundtrip(&card);
    assert!(card.links.as_ref().unwrap().contains_key("link3"));
}

#[test]
fn rfc9553_sec_2_6_4_media() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.6.4_media.json");
    assert!(note.contains("RFC 9553 §2.6.4"));
    assert_roundtrip(&card);
    assert_eq!(card.media.as_ref().unwrap().len(), 3);
}

#[test]
fn rfc9553_sec_2_7_1_localization_top_level() {
    let (note, card, _) =
        load_rfc_fixture::<ContactCard>("rfc9553/2.7.1_localization_top_level.json");
    assert!(note.contains("RFC 9553 §2.7.1"));
    assert_roundtrip(&card);
    assert!(card.localizations.is_some());
}

#[test]
fn rfc9553_sec_2_7_1_localization_nested() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.7.1_localization_nested.json");
    assert!(note.contains("RFC 9553 §2.7.1"));
    assert_roundtrip(&card);
    assert!(card.localizations.is_some());
    assert!(card.titles.is_some());
}

#[test]
fn rfc9553_sec_2_8_1_anniversaries() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.8.1_anniversaries.json");
    assert!(note.contains("RFC 9553 §2.8.1"));
    assert_roundtrip(&card);
    assert_eq!(card.anniversaries.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_8_2_keywords() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.8.2_keywords.json");
    assert!(note.contains("RFC 9553 §2.8.2"));
    assert_roundtrip(&card);
    assert_eq!(card.keywords.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc9553_sec_2_8_3_notes() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.8.3_notes.json");
    assert!(note.contains("RFC 9553 §2.8.3"));
    assert_roundtrip(&card);
    assert!(card.notes.as_ref().unwrap().contains_key("n1"));
}

#[test]
fn rfc9553_sec_2_8_4_personal_info() {
    let (note, card, _) = load_rfc_fixture::<ContactCard>("rfc9553/2.8.4_personal_info.json");
    assert!(note.contains("RFC 9553 §2.8.4"));
    assert_roundtrip(&card);
    assert_eq!(card.personal_info.as_ref().unwrap().len(), 3);
}

// ===========================================================================
// RFC 8984 JSCalendar Specifications (July 2021)
// ===========================================================================

#[test]
fn rfc8984_sec_6_1_simple_event() {
    let (note, event, _) = load_rfc_fixture::<CalendarEvent>("rfc8984/6.1_simple_event.json");
    assert!(note.contains("RFC 8984 §6.1"));
    assert_roundtrip(&event);
    assert_eq!(event.title.as_deref(), Some("Some event"));
    assert_eq!(event.duration.as_deref(), Some("PT1H"));
}

#[test]
fn rfc8984_sec_6_2_simple_task() {
    let (note, task, _) = load_rfc_fixture::<Task>("rfc8984/6.2_simple_task.json");
    assert!(note.contains("RFC 8984 §6.2"));
    assert_roundtrip(&task);
    assert_eq!(task.title.as_deref(), Some("Do something"));
}

#[test]
fn rfc8984_sec_6_3_simple_group() {
    let (note, group, _) = load_rfc_fixture::<CalendarGroup>("rfc8984/6.3_simple_group.json");
    assert!(note.contains("RFC 8984 §6.3"));
    assert_roundtrip(&group);
    assert_eq!(
        group.uid.as_deref(),
        Some("bf0ac22b-4989-4caf-9ebd-54301b4ee51a")
    );
    assert!(group.entries.is_some());
}

#[test]
fn rfc8984_sec_6_4_all_day_event() {
    let (note, event, _) = load_rfc_fixture::<CalendarEvent>("rfc8984/6.4_all_day_event.json");
    assert!(note.contains("RFC 8984 §6.4"));
    assert_roundtrip(&event);
    assert_eq!(event.title.as_deref(), Some("April Fool's Day"));
    assert_eq!(event.show_without_time, Some(true));
}

#[test]
fn rfc8984_sec_6_5_task_due_date() {
    let (note, task, _) = load_rfc_fixture::<Task>("rfc8984/6.5_task_due_date.json");
    assert!(note.contains("RFC 8984 §6.5"));
    assert_roundtrip(&task);
    assert_eq!(task.title.as_deref(), Some("Buy groceries"));
    assert_eq!(task.due.as_deref(), Some("2020-01-19T18:00:00"));
}

#[test]
fn rfc8984_sec_6_6_event_end_time_zone() {
    let (note, event, _) =
        load_rfc_fixture::<CalendarEvent>("rfc8984/6.6_event_end_time_zone.json");
    assert!(note.contains("RFC 8984 §6.6"));
    assert_roundtrip(&event);
    assert_eq!(event.title.as_deref(), Some("Flight XY51 to Tokyo"));
    assert_eq!(event.locations.as_ref().unwrap().len(), 2);
}

#[test]
fn rfc8984_sec_6_7_floating_time_event() {
    let (note, event, _) =
        load_rfc_fixture::<CalendarEvent>("rfc8984/6.7_floating_time_event.json");
    assert!(note.contains("RFC 8984 §6.7"));
    assert_roundtrip(&event);
    assert_eq!(event.title.as_deref(), Some("Yoga"));
    assert_eq!(event.duration.as_deref(), Some("PT30M"));
}

#[test]
fn rfc8984_sec_6_8_multiple_locations() {
    let (note, event, _) = load_rfc_fixture::<CalendarEvent>("rfc8984/6.8_multiple_locations.json");
    assert!(note.contains("RFC 8984 §6.8"));
    assert_eq!(
        event.title.as_deref(),
        Some("Live from Music Bowl: The Band")
    );
    assert!(event.locations.is_some());
    assert!(event.virtual_locations.is_some());
    assert!(event.localizations.is_some());

    // Under JSCalendar-bis §7.4.2.2, reserved property localizations is tolerated
    // on input but never re-emitted on serialization.
    let serialized = serde_json::to_string_pretty(&event).expect("serialization succeeds");
    let roundtripped: CalendarEvent =
        serde_json::from_str(&serialized).expect("deserialization succeeds");
    assert_eq!(event.title, roundtripped.title);
    assert!(roundtripped.localizations.is_none());
}

#[test]
fn rfc8984_sec_6_9_recurring_event_overrides() {
    let (note, event, _) =
        load_rfc_fixture::<CalendarEvent>("rfc8984/6.9_recurring_event_overrides.json");
    assert!(note.contains("RFC 8984 §6.9"));
    assert_roundtrip(&event);
    assert_eq!(event.title.as_deref(), Some("Calculus I"));
    assert_eq!(event.recurrence_overrides.as_ref().unwrap().len(), 3);
}

#[test]
fn rfc8984_sec_6_10_recurring_event_participants() {
    let (note, event, _) =
        load_rfc_fixture::<CalendarEvent>("rfc8984/6.10_recurring_event_participants.json");
    assert!(note.contains("RFC 8984 §6.10"));
    assert_roundtrip(&event);
    assert_eq!(event.title.as_deref(), Some("FooBar team meeting"));
    assert_eq!(event.participants.as_ref().unwrap().len(), 2);
    assert!(event.virtual_locations.is_some());
}

// ===========================================================================
// draft-ietf-jmap-calendars (October 2026 / draft-32)
// ===========================================================================

#[test]
fn jmap_calendars_sec_5_9_1_calendar_event() {
    let (note, event, _) =
        load_rfc_fixture::<CalendarEvent>("jmap_calendars/5.9.1_calendar_event.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §5.9.1"));
    assert_roundtrip(&event);
    assert_eq!(event.id.as_ref().map(|id| id.as_str()), Some("123"));
    assert_eq!(event.title.as_deref(), Some("FooBar team meeting"));
    assert!(event.recurrence_rule.is_some());
    assert_eq!(event.participants.as_ref().unwrap().len(), 2);
}

#[test]
fn jmap_calendars_sec_5_9_1_patch_zoe_decline() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/5.9.1_patch_zoe_decline.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §5.9.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/set");
}

#[test]
fn jmap_calendars_sec_5_9_1_recurrence_overrides() {
    let (note, event, _) =
        load_rfc_fixture::<CalendarEvent>("jmap_calendars/5.9.1_recurrence_overrides.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §5.9.1"));
    assert_roundtrip(&event);
    assert!(event.recurrence_overrides.is_some());
}

#[test]
fn jmap_calendars_sec_5_9_1_patch_tom_restore() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/5.9.1_patch_tom_restore.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §5.9.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/set");
}

#[test]
fn jmap_calendars_sec_5_9_1_patch_invalid_remove() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/5.9.1_patch_invalid_remove.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §5.9.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/set");
}

#[test]
fn jmap_calendars_sec_5_9_1_patch_tom_remove_instance() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/5.9.1_patch_tom_remove_instance.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §5.9.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/set");
}

#[test]
fn jmap_calendars_sec_8_1_initial_data_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.1_initial_data_request.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].name, "Calendar/get");
    assert_eq!(calls[1].name, "ParticipantIdentity/get");
    assert_eq!(calls[2].name, "CalendarEvent/query");
    assert_eq!(calls[3].name, "CalendarEvent/get");
}

#[test]
fn jmap_calendars_sec_8_1_initial_data_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.1_initial_data_response.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 4);
    assert_eq!(responses[0].name, "Calendar/get");
    assert_eq!(responses[1].name, "ParticipantIdentity/get");
    assert_eq!(responses[2].name, "CalendarEvent/query");
    assert_eq!(responses[3].name, "CalendarEvent/get");
}

#[test]
fn jmap_calendars_sec_8_2_create_event_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.2_create_event_request.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.2"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/set");

    let set: CalendarEventSetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into CalendarEventSetRequest");
    assert_eq!(set.send_scheduling_messages, Some(true));
    let draft = &set.set.create.unwrap()["k559"];
    assert_eq!(draft.title.as_deref(), Some("Party at Pete's"));
}

#[test]
fn jmap_calendars_sec_8_2_create_event_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.2_create_event_response.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.2"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "CalendarEvent/set");

    let res: SetResponse<CalendarEvent> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<CalendarEvent>");
    let created = &res.created.unwrap()["k559"];
    assert_eq!(
        created.id.as_ref().map(|id| id.as_str()),
        Some("E-5d5776f6-ff8e-4bfd-ab3e-fe2fe5d4fa91")
    );
    assert_eq!(created.is_origin, Some(true));
}

#[test]
fn jmap_calendars_sec_8_3_push_alert_notification() {
    let (note, alert, _) =
        load_rfc_fixture::<CalendarAlert>("jmap_calendars/8.3_push_alert_notification.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.3"));
    assert_roundtrip(&alert);
    assert_eq!(alert.account_id.as_str(), "a0x9");
    assert_eq!(
        alert.calendar_event_id.as_str(),
        "E-7e93e3ee-4e6e-408a-9adc-cbaf1dbd0a3f"
    );
    assert_eq!(
        alert.alert_id.as_str(),
        "7519a951-1e6f-4a6c-b08b-20dd2e5a89cd"
    );
}

#[test]
fn jmap_calendars_sec_8_3_get_event_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.3_get_event_request.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.3"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/get");

    let get: GetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(
        get.ids,
        Some(vec![Id::from("E-7e93e3ee-4e6e-408a-9adc-cbaf1dbd0a3f")])
    );
}

#[test]
fn jmap_calendars_sec_8_3_get_event_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.3_get_event_response.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.3"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "CalendarEvent/get");

    let res: GetResponse<CalendarEvent> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<CalendarEvent>");
    assert_eq!(res.list.len(), 1);
    assert_eq!(
        res.list[0].id.as_ref().map(|id| id.as_str()),
        Some("E-7e93e3ee-4e6e-408a-9adc-cbaf1dbd0a3f")
    );
}

#[test]
fn jmap_calendars_sec_8_3_snooze_alert_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.3_snooze_alert_request.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.3"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/set");
}

#[test]
fn jmap_calendars_sec_8_4_default_calendar_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.4_default_calendar_request.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.4"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Calendar/set");

    let set: CalendarSetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into CalendarSetRequest");
    assert_eq!(
        set.on_success_set_is_default,
        Some(Id::from("062adcfa-105d-455c-bc60-6db68b69c3f3"))
    );
}

#[test]
fn jmap_calendars_sec_8_4_default_calendar_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.4_default_calendar_response.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.4"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Calendar/set");

    let res: SetResponse<Calendar> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<Calendar>");
    assert!(res.updated.is_some());
}

#[test]
fn jmap_calendars_sec_8_5_parse_ical_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.5_parse_ical_request.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.5"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "CalendarEvent/parse");

    let parse: CalendarEventParseRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into CalendarEventParseRequest");
    assert_eq!(
        parse.blob_ids,
        vec![Id::from("Ge682d5d7aad50b3a4f7180a7ed9276476485ea52")]
    );
}

#[test]
fn jmap_calendars_sec_8_5_parse_ical_parsed_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("jmap_calendars/8.5_parse_ical_parsed_response.json");
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.5"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "CalendarEvent/parse");
}

#[test]
fn jmap_calendars_sec_8_5_parse_ical_not_found_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>(
        "jmap_calendars/8.5_parse_ical_not_found_response.json",
    );
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.5"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "CalendarEvent/parse");
}

#[test]
fn jmap_calendars_sec_8_5_parse_ical_not_parsable_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>(
        "jmap_calendars/8.5_parse_ical_not_parsable_response.json",
    );
    assert!(note.contains("draft-ietf-jmap-calendars-32 §8.5"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "CalendarEvent/parse");
}

// ===========================================================================
// RFC 9425 JMAP for Quotas (June 2023)
// ===========================================================================

#[test]
fn rfc9425_sec_5_1_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9425/5.1_get_request.json");
    assert!(note.contains("RFC 9425 §5.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Quota/get");

    let req: GetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(req.account_id, Id::from("u33084183"));
    assert_eq!(req.ids, None);
}

#[test]
fn rfc9425_sec_5_1_get_response() {
    let (note, responses, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9425/5.1_get_response.json");
    assert!(note.contains("RFC 9425 §5.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Quota/get");

    let res: GetResponse<Quota> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<Quota>");
    assert_eq!(res.list.len(), 2);
    assert_eq!(
        res.list[0].id,
        Id::from("2a06df0d-9865-4e74-a92f-74dcc814270e")
    );
    assert_eq!(res.list[0].resource_type, "count");
    assert_eq!(res.list[0].used, 1056);
    assert_eq!(res.list[0].warn_limit, Some(1600));
    assert_eq!(res.list[0].soft_limit, Some(1800));
    assert_eq!(res.list[0].hard_limit, 2000);
    assert_eq!(res.list[0].scope, "account");
    assert_eq!(res.list[0].types, vec!["Mail", "Calendar", "Contact"]);
    assert_eq!(res.list[1].resource_type, "octets");
    assert_eq!(res.list[1].used, 524288000);
}

#[test]
fn rfc9425_sec_5_2_changes_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9425/5.2_changes_request.json");
    assert!(note.contains("RFC 9425 §5.2"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "Quota/changes");
    assert_eq!(calls[1].name, "Quota/get");

    let changes: ChangesRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("call 0 must deserialize into ChangesRequest");
    assert_eq!(changes.since_state, "78540".into());
    assert_eq!(changes.max_changes, Some(20));

    let get: GetRequest = serde_json::from_value(calls[1].arguments.clone())
        .expect("call 1 must deserialize into GetRequest");
    assert_eq!(get.account_id, Id::from("u33084183"));
}

#[test]
fn rfc9425_sec_5_2_changes_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9425/5.2_changes_response.json");
    assert!(note.contains("RFC 9425 §5.2"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Quota/changes");
    assert_eq!(responses[1].name, "Quota/get");

    let changes: ChangesResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("response 0 must deserialize into ChangesResponse");
    assert_eq!(changes.old_state, "78540".into());
    assert_eq!(changes.new_state, "78542".into());
    assert_eq!(
        changes.updated,
        vec![Id::from("2a06df0d-9865-4e74-a92f-74dcc814270e")]
    );

    let get: GetResponse<Quota> = serde_json::from_value(responses[1].arguments.clone())
        .expect("response 1 must deserialize into GetResponse<Quota>");
    assert_eq!(get.list.len(), 1);
    assert_eq!(
        get.list[0].id,
        Id::from("2a06df0d-9865-4e74-a92f-74dcc814270e")
    );
    assert_eq!(get.list[0].used, 1246);
}

// ===========================================================================
// RFC 9670 JMAP Sharing (November 2024)
// ===========================================================================

#[test]
fn rfc9670_sec_4_1_session_object() {
    let (note, session, _) = load_rfc_fixture::<Session>("rfc9670/4.1_session_object.json");
    assert!(note.contains("RFC 9670 §4.1"));
    assert_roundtrip(&session);
    let account = session
        .accounts
        .get(&Id::from("u12345678"))
        .expect("account must exist");
    assert_eq!(account.name, "jane.doe@example.com");
    assert!(account.is_personal);
    let owner_cap = account
        .account_capabilities
        .get("urn:ietf:params:jmap:principals:owner")
        .expect("owner capability must be present");
    assert_eq!(owner_cap["accountIdForPrincipal"], "u33084183");
    assert_eq!(owner_cap["principalId"], "P105aga511jaa");
}

#[test]
fn rfc9670_sec_4_1_principal_get_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9670/4.1_principal_get_request.json");
    assert!(note.contains("RFC 9670 §4.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Principal/get");

    let req: GetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(req.account_id, Id::from("u33084183"));
    assert_eq!(req.ids, None);
}

#[test]
fn rfc9670_sec_4_1_principal_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9670/4.1_principal_get_response.json");
    assert!(note.contains("RFC 9670 §4.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "Principal/get");

    let res: GetResponse<Principal> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<Principal>");
    assert_eq!(res.list.len(), 2);
    assert_eq!(res.list[0].id, Some(Id::from("P2342fnddd20")));
    assert_eq!(res.list[0].name, "Joe Bloggs");
    assert_eq!(res.list[0].principal_type, Some("individual".to_string()));
    assert_eq!(
        res.list[0].email,
        Some("joe.bloggs@example.com".to_string())
    );
    assert_eq!(
        res.list[0].time_zone,
        Some("Australia/Melbourne".to_string())
    );
    assert_eq!(res.list[1].id, Some(Id::from("P674pp24095qo49pr")));
    assert_eq!(res.list[1].name, "Board room");
    assert_eq!(res.list[1].principal_type, Some("location".to_string()));
}

#[test]
fn rfc9670_sec_4_1_todolist_set_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9670/4.1_todolist_set_request.json");
    assert!(note.contains("RFC 9670 §4.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "TodoList/set");

    let req: SetRequest<Value> = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SetRequest<Value>");
    assert_eq!(req.account_id, Id::from("u12345678"));
    assert!(req.update.is_some());
}

// ===========================================================================
// RFC 9661 JMAP for Sieve Scripts (September 2024)
// ===========================================================================

#[test]
fn rfc9661_sec_1_2_2_session() {
    let (note, session, _) = load_rfc_fixture::<Session>("rfc9661/1.2.2_session.json");
    assert!(note.contains("RFC 9661 §1.2.2"));
    assert_roundtrip(&session);
    let account = session
        .accounts
        .get(&Id::from("ken"))
        .expect("account ken must exist");
    assert_eq!(account.name, "ken@example.com");
    let sieve_cap = account
        .account_capabilities
        .get("urn:ietf:params:jmap:sieve")
        .expect("sieve capability must be present");
    let cap: SieveCapability = serde_json::from_value(sieve_cap.clone())
        .expect("sieve capability must deserialize into SieveCapability");
    assert_eq!(cap.max_size_script, 65536);
    assert_eq!(cap.max_number_scripts, Some(5));
    assert!(cap.sieve_extensions.contains(&"fileinto".to_string()));
}

#[test]
fn rfc9661_sec_2_3_1_get_all_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.3.1_get_all_request.json");
    assert!(note.contains("RFC 9661 §2.3.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "SieveScript/get");

    let req: GetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into GetRequest");
    assert_eq!(req.account_id, Id::from("ken"));
}

#[test]
fn rfc9661_sec_2_3_1_get_all_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.3.1_get_all_response.json");
    assert!(note.contains("RFC 9661 §2.3.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "SieveScript/get");

    let res: GetResponse<SieveScript> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into GetResponse<SieveScript>");
    assert_eq!(res.list.len(), 1);
    assert_eq!(
        res.list[0].id,
        Some(Id::from("2d647053-dded-418d-917a-63eda3ac8f7b"))
    );
    assert_eq!(res.list[0].name, "test1");
    assert!(res.list[0].is_active);
    assert_eq!(res.list[0].blob_id, Id::from("S7"));
}

#[test]
fn rfc9661_sec_2_3_1_get_and_blob_get_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.3.1_get_and_blob_get_request.json");
    assert!(note.contains("RFC 9661 §2.3.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "SieveScript/get");
    assert_eq!(calls[1].name, "Blob/get");

    let sieve_get: GetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("call 0 must deserialize into GetRequest");
    assert_eq!(
        sieve_get.ids,
        Some(vec![Id::from("2d647053-dded-418d-917a-63eda3ac8f7b")])
    );

    let blob_get: BlobGetRequest = serde_json::from_value(calls[1].arguments.clone())
        .expect("call 1 must deserialize into BlobGetRequest");
    assert_eq!(blob_get.account_id, Id::from("ken"));
    assert!(blob_get.ids_ref.is_some());
}

#[test]
fn rfc9661_sec_2_3_1_get_and_blob_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.3.1_get_and_blob_get_response.json");
    assert!(note.contains("RFC 9661 §2.3.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "SieveScript/get");
    assert_eq!(responses[1].name, "Blob/get");

    let sieve_res: GetResponse<SieveScript> =
        serde_json::from_value(responses[0].arguments.clone())
            .expect("response 0 must deserialize into GetResponse<SieveScript>");
    assert_eq!(sieve_res.list.len(), 1);

    let blob_res: BlobGetResponse = serde_json::from_value(responses[1].arguments.clone())
        .expect("response 1 must deserialize into BlobGetResponse");
    assert_eq!(blob_res.list.len(), 1);
    assert_eq!(blob_res.list[0].id, Id::from("S7"));
    assert_eq!(blob_res.list[0].size, Some(49));
    assert!(
        blob_res.list[0]
            .data_as_text
            .as_ref()
            .unwrap()
            .contains("fileinto")
    );
}

#[test]
fn rfc9661_sec_2_4_1_upload_response() {
    let (note, val, _) = load_rfc_fixture::<Value>("rfc9661/2.4.1_upload_response.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&val);
    assert_eq!(val["accountId"], "ken");
    assert_eq!(val["blobId"], "Gabcc83e44a6e19991c4568d0b94e1767c83dd123");
    assert_eq!(val["type"], "application/sieve");
    assert_eq!(val["size"], 98);
}

#[test]
fn rfc9661_sec_2_4_1_create_activate_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_create_activate_request.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "SieveScript/set");

    let set: SieveScriptSetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SieveScriptSetRequest");
    assert_eq!(
        set.on_success_activate_script,
        Some(Value::String("#A".to_string()))
    );
}

#[test]
fn rfc9661_sec_2_4_1_create_activate_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_create_activate_response.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "SieveScript/set");

    let res: SetResponse<SieveScript> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<SieveScript>");
    assert!(res.created.is_some());
    assert!(res.updated.is_some());
}

#[test]
fn rfc9661_sec_2_4_1_update_blob_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_update_blob_request.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "Blob/upload");
    assert_eq!(calls[1].name, "SieveScript/set");

    let upload: BlobUploadRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("call 0 must deserialize into BlobUploadRequest");
    assert_eq!(upload.create.len(), 1);

    let set: SieveScriptSetRequest = serde_json::from_value(calls[1].arguments.clone())
        .expect("call 1 must deserialize into SieveScriptSetRequest");
    assert!(set.set.update.is_some());
}

#[test]
fn rfc9661_sec_2_4_1_update_blob_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_update_blob_response.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Blob/upload");
    assert_eq!(responses[1].name, "SieveScript/set");

    let upload_res: BlobUploadResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("response 0 must deserialize into BlobUploadResponse");
    assert!(upload_res.created.is_some());

    let set_res: SetResponse<SieveScript> = serde_json::from_value(responses[1].arguments.clone())
        .expect("response 1 must deserialize into SetResponse<SieveScript>");
    assert!(set_res.updated.is_some());
}

#[test]
fn rfc9661_sec_2_4_1_rename_deactivate_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_rename_deactivate_request.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "SieveScript/set");

    let set: SieveScriptSetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SieveScriptSetRequest");
    assert_eq!(set.on_success_deactivate_script, Some(true));
}

#[test]
fn rfc9661_sec_2_4_1_rename_deactivate_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_rename_deactivate_response.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "SieveScript/set");

    let res: SetResponse<SieveScript> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<SieveScript>");
    assert!(res.updated.is_some());
}

#[test]
fn rfc9661_sec_2_4_1_reactivate_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_reactivate_request.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "SieveScript/set");

    let set: SieveScriptSetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("arguments must deserialize into SieveScriptSetRequest");
    assert_eq!(
        set.on_success_activate_script,
        Some(Value::String("dd1b164f-8cdc-448c-9f54".to_string()))
    );
}

#[test]
fn rfc9661_sec_2_4_1_reactivate_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_reactivate_response.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].name, "SieveScript/set");

    let res: SetResponse<SieveScript> = serde_json::from_value(responses[0].arguments.clone())
        .expect("arguments must deserialize into SetResponse<SieveScript>");
    assert!(res.updated.is_some());
}

#[test]
fn rfc9661_sec_2_4_1_deactivate_destroy_request() {
    let (note, calls, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_deactivate_destroy_request.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "SieveScript/set");
    assert_eq!(calls[1].name, "SieveScript/set");

    let set0: SieveScriptSetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("call 0 must deserialize into SieveScriptSetRequest");
    assert_eq!(set0.on_success_deactivate_script, Some(true));

    let set1: SieveScriptSetRequest = serde_json::from_value(calls[1].arguments.clone())
        .expect("call 1 must deserialize into SieveScriptSetRequest");
    assert!(set1.set.destroy.is_some());
}

#[test]
fn rfc9661_sec_2_4_1_deactivate_destroy_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9661/2.4.1_deactivate_destroy_response.json");
    assert!(note.contains("RFC 9661 §2.4.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "SieveScript/set");
    assert_eq!(responses[1].name, "SieveScript/set");

    let res0: SetResponse<SieveScript> = serde_json::from_value(responses[0].arguments.clone())
        .expect("response 0 must deserialize into SetResponse<SieveScript>");
    assert!(res0.updated.is_some());

    let res1: SetResponse<SieveScript> = serde_json::from_value(responses[1].arguments.clone())
        .expect("response 1 must deserialize into SetResponse<SieveScript>");
    assert!(res1.destroyed.is_some());
}

// ===========================================================================
// RFC 9404 JMAP Blob Management Extension (August 2023)
// ===========================================================================

#[test]
fn rfc9404_sec_3_1_1_capability() {
    let (note, session, _) = load_rfc_fixture::<Session>("rfc9404/3.1.1_capability.json");
    assert!(note.contains("RFC 9404 §3.1.1"));
    assert_roundtrip(&session);
    let account = session
        .accounts
        .get(&Id::from("A13842"))
        .expect("account A13842 must exist");
    let blob_cap = account
        .blob_capability()
        .expect("blob capability must be present");
    assert_eq!(blob_cap.max_size_blob_set, Some(50000000));
    assert_eq!(blob_cap.max_data_sources, Some(100));
    assert_eq!(
        blob_cap.supported_type_names,
        Some(vec![
            "Mailbox".to_string(),
            "Thread".to_string(),
            "Email".to_string()
        ])
    );
    assert_eq!(
        blob_cap.supported_digest_algorithms,
        Some(vec!["sha".to_string(), "sha-256".to_string()])
    );
}

#[test]
fn rfc9404_sec_4_1_1_upload_request() {
    let (note, call, _) = load_rfc_fixture::<Invocation>("rfc9404/4.1.1_upload_request.json");
    assert!(note.contains("RFC 9404 §4.1.1"));
    assert_roundtrip(&call);
    assert_eq!(call.name, "Blob/upload");

    let req: BlobUploadRequest = serde_json::from_value(call.arguments)
        .expect("arguments must deserialize into BlobUploadRequest");
    assert_eq!(req.account_id, Id::from("account1"));
    assert_eq!(req.create.len(), 1);
    assert_eq!(req.create["1"].content_type, Some("image/png".to_string()));
}

#[test]
fn rfc9404_sec_4_1_1_upload_response() {
    let (note, res_inv, _) = load_rfc_fixture::<Invocation>("rfc9404/4.1.1_upload_response.json");
    assert!(note.contains("RFC 9404 §4.1.1"));
    assert_roundtrip(&res_inv);
    assert_eq!(res_inv.name, "Blob/upload");

    let res: BlobUploadResponse = serde_json::from_value(res_inv.arguments)
        .expect("arguments must deserialize into BlobUploadResponse");
    assert_eq!(res.account_id, Id::from("account1"));
    let created = res.created.expect("created must be present");
    assert_eq!(
        created["1"].id,
        Id::from("G4c6751edf9dd6903ff54b792e432fba781271beb")
    );
    assert_eq!(created["1"].size, 95);
    assert_eq!(created["1"].content_type, Some("image/png".to_string()));
}

#[test]
fn rfc9404_sec_4_1_2_upload_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9404/4.1.2_upload_request.json");
    assert!(note.contains("RFC 9404 §4.1.2"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].name, "Blob/upload");
    assert_eq!(calls[1].name, "Blob/upload");
    assert_eq!(calls[2].name, "Blob/get");

    let upload0: BlobUploadRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("call 0 must deserialize into BlobUploadRequest");
    assert!(upload0.account_id.is_empty());
    assert_eq!(upload0.create.len(), 1);

    let upload1: BlobUploadRequest = serde_json::from_value(calls[1].arguments.clone())
        .expect("call 1 must deserialize into BlobUploadRequest");
    assert_eq!(upload1.create.len(), 1);

    let get: BlobGetRequest = serde_json::from_value(calls[2].arguments.clone())
        .expect("call 2 must deserialize into BlobGetRequest");
    assert_eq!(get.ids, vec![Id::from("#cat")]);
}

#[test]
fn rfc9404_sec_4_1_2_upload_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9404/4.1.2_upload_response.json");
    assert!(note.contains("RFC 9404 §4.1.2"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0].name, "Blob/upload");
    assert_eq!(responses[1].name, "Blob/upload");
    assert_eq!(responses[2].name, "Blob/get");

    let upload_res0: BlobUploadResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("response 0 must deserialize into BlobUploadResponse");
    assert!(upload_res0.created.is_some());

    let upload_res1: BlobUploadResponse = serde_json::from_value(responses[1].arguments.clone())
        .expect("response 1 must deserialize into BlobUploadResponse");
    assert!(upload_res1.created.is_some());

    let get_res: BlobGetResponse = serde_json::from_value(responses[2].arguments.clone())
        .expect("response 2 must deserialize into BlobGetResponse");
    assert_eq!(get_res.list.len(), 1);
    assert_eq!(
        get_res.list[0].data_as_text,
        Some("How quick was that?".to_string())
    );
}

#[test]
fn rfc9404_sec_4_2_1_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9404/4.2.1_get_request.json");
    assert!(note.contains("RFC 9404 §4.2.1"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "Blob/get");
    assert_eq!(calls[1].name, "Blob/get");

    let get0: BlobGetRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("call 0 must deserialize into BlobGetRequest");
    assert_eq!(get0.ids.len(), 2);

    let get1: BlobGetRequest = serde_json::from_value(calls[1].arguments.clone())
        .expect("call 1 must deserialize into BlobGetRequest");
    assert_eq!(get1.offset, Some(4));
    assert_eq!(get1.length, Some(9));
}

#[test]
fn rfc9404_sec_4_2_1_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9404/4.2.1_get_response.json");
    assert!(note.contains("RFC 9404 §4.2.1"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].name, "Blob/get");
    assert_eq!(responses[1].name, "Blob/get");

    let res0: BlobGetResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("response 0 must deserialize into BlobGetResponse");
    assert_eq!(res0.list.len(), 1);
    assert_eq!(res0.not_found, vec![Id::from("not-a-blob")]);

    let res1: BlobGetResponse = serde_json::from_value(responses[1].arguments.clone())
        .expect("response 1 must deserialize into BlobGetResponse");
    assert_eq!(res1.list.len(), 1);
    assert_eq!(res1.list[0].data_as_text, Some("quick bro".to_string()));
}

#[test]
fn rfc9404_sec_4_2_2_get_request() {
    let (note, calls, _) = load_rfc_fixture::<Vec<Invocation>>("rfc9404/4.2.2_get_request.json");
    assert!(note.contains("RFC 9404 §4.2.2"));
    assert_roundtrip(&calls);
    assert_eq!(calls.len(), 6);
    assert_eq!(calls[0].name, "Blob/upload");
    for call in &calls[1..] {
        assert_eq!(call.name, "Blob/get");
    }

    let upload: BlobUploadRequest = serde_json::from_value(calls[0].arguments.clone())
        .expect("call 0 must deserialize into BlobUploadRequest");
    assert_eq!(upload.create.len(), 2);
}

#[test]
fn rfc9404_sec_4_2_2_get_response() {
    let (note, responses, _) =
        load_rfc_fixture::<Vec<Invocation>>("rfc9404/4.2.2_get_response.json");
    assert!(note.contains("RFC 9404 §4.2.2"));
    assert_roundtrip(&responses);
    assert_eq!(responses.len(), 6);
    assert_eq!(responses[0].name, "Blob/upload");
    for res in &responses[1..] {
        assert_eq!(res.name, "Blob/get");
    }

    let upload_res: BlobUploadResponse = serde_json::from_value(responses[0].arguments.clone())
        .expect("response 0 must deserialize into BlobUploadResponse");
    assert!(upload_res.created.is_some());

    let g1_res: BlobGetResponse = serde_json::from_value(responses[1].arguments.clone())
        .expect("G1 must deserialize into BlobGetResponse");
    assert_eq!(g1_res.list.len(), 2);
    assert_eq!(g1_res.list[0].is_encoding_problem, Some(true));

    let g5_res: BlobGetResponse = serde_json::from_value(responses[5].arguments.clone())
        .expect("G5 must deserialize into BlobGetResponse");
    assert_eq!(g5_res.list[0].is_truncated, Some(true));
    assert_eq!(g5_res.list[1].is_truncated, Some(true));
}

#[test]
fn rfc9404_sec_4_3_1_lookup_request() {
    let (note, call, _) = load_rfc_fixture::<Invocation>("rfc9404/4.3.1_lookup_request.json");
    assert!(note.contains("RFC 9404 §4.3.1"));
    assert_roundtrip(&call);
    assert_eq!(call.name, "Blob/lookup");

    let req: BlobLookupRequest = serde_json::from_value(call.arguments)
        .expect("arguments must deserialize into BlobLookupRequest");
    assert_eq!(req.type_names, vec!["Mailbox", "Thread", "Email"]);
    assert_eq!(req.ids.len(), 2);
}

#[test]
fn rfc9404_sec_4_3_1_lookup_response() {
    let (note, res_inv, _) = load_rfc_fixture::<Invocation>("rfc9404/4.3.1_lookup_response.json");
    assert!(note.contains("RFC 9404 §4.3.1"));
    assert_roundtrip(&res_inv);
    assert_eq!(res_inv.name, "Blob/lookup");

    let res: BlobLookupResponse = serde_json::from_value(res_inv.arguments)
        .expect("arguments must deserialize into BlobLookupResponse");
    assert_eq!(res.list.len(), 1);
    assert_eq!(res.not_found, vec![Id::from("not-a-blob")]);
    assert_eq!(
        res.list[0].id,
        Id::from("Gd2f81008cf07d2425418f7f02a3ca63a8bc82003")
    );
    assert_eq!(
        res.list[0].matched_ids["Mailbox"],
        vec![Id::from("M54e97373"), Id::from("Mcbe6b662")]
    );
}
