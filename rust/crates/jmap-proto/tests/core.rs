// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the RFC 8620 core types against fixture JSON taken
//! from the RFC examples. Comparison happens on `serde_json::Value` so a
//! field silently dropped or renamed by our types fails the test.

use jmap_proto::error::{MethodError, RequestError};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::response::Response;
use jmap_proto::session::Session;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn roundtrip<T>(value: &Value) -> Value
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let typed: T = serde_json::from_value(value.clone()).expect("deserialize");
    serde_json::to_value(&typed).expect("serialize")
}

#[test]
fn request_envelope_roundtrip() {
    let value = fixture("core/request.json");
    assert_eq!(roundtrip::<Request>(&value), value);

    let request: Request = serde_json::from_value(value).unwrap();
    assert_eq!(request.using.len(), 2);
    assert_eq!(request.method_calls.len(), 3);
    assert_eq!(request.method_calls[0].name, "method1");
    assert_eq!(request.method_calls[2].call_id, "c3");
}

#[test]
fn error_response_roundtrip() {
    let value = fixture("core/response_with_error.json");
    assert_eq!(roundtrip::<Response>(&value), value);

    let response: Response = serde_json::from_value(value).unwrap();
    assert_eq!(response.session_state.as_str(), "75128aab4b1b");
    assert_eq!(response.method_responses.len(), 4);

    let error_invocation = &response.method_responses[3];
    assert!(error_invocation.is_error());
    let error: MethodError = error_invocation.parse().unwrap();
    assert_eq!(error.error_type, "unknownMethod");

    // Request-level errors are RFC 7807 problem details; extension members
    // (here: "limit") must survive a round-trip.
    let problem = fixture("core/problem_details.json");
    assert_eq!(roundtrip::<RequestError>(&problem), problem);
    let error: RequestError = serde_json::from_value(problem).unwrap();
    assert_eq!(error.error_type, "urn:ietf:params:jmap:error:limit");
    assert_eq!(error.status, Some(400));
}

#[test]
fn result_reference_serialization() {
    let value = fixture("core/result_reference.json");
    assert_eq!(roundtrip::<Request>(&value), value);

    let request: Request = serde_json::from_value(value).unwrap();
    let arguments = &request.method_calls[1].arguments;
    let reference: ResultReference =
        serde_json::from_value(arguments.get("#ids").unwrap().clone()).unwrap();
    assert_eq!(reference.result_of, "t0");
    assert_eq!(reference.name, "Foo/changes");
    assert_eq!(reference.path, "/created");
}

#[test]
fn session_object_roundtrip() {
    let value = fixture("core/session.json");
    assert_eq!(roundtrip::<Session>(&value), value);

    let session: Session = serde_json::from_value(value).unwrap();
    assert!(
        session
            .capabilities
            .contains_key(jmap_proto::session::CAPABILITY_MAIL)
    );
    assert_eq!(
        session.primary_accounts[jmap_proto::session::CAPABILITY_MAIL].as_str(),
        "A13824"
    );
    let account = session.accounts.values().next().unwrap();
    assert_eq!(account.name, "john@example.com");
    assert!(account.is_personal);
    assert!(!account.is_read_only);
}

/// The largest thing this server will take (RFC 8620 §6.1), read off the core
/// capability — the number a client has to know *before* it sends a message,
/// because the alternative to reading it is finding out by uploading.
#[test]
fn the_session_names_the_largest_upload_the_server_takes() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(session.max_size_upload(), Some(50_000_000));
}

/// How many method calls one request may carry (RFC 8620 §2), which is what
/// decides whether a chained `Email/query` + `Email/get` may go in one request
/// or has to be two.
#[test]
fn the_session_names_how_many_calls_one_request_may_carry() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(session.max_calls_in_request(), Some(32));
}

/// A server that names no call limit is answered with `None`, like the other
/// two: what to do without a number is the caller's decision, and a guess here
/// would either split requests that were fine or send ones that are refused.
#[test]
fn a_session_that_names_no_call_limit_says_so() {
    let mut value = fixture("core/session.json");
    value["capabilities"][jmap_proto::session::CAPABILITY_CORE]
        .as_object_mut()
        .unwrap()
        .remove("maxCallsInRequest");
    let session: Session = serde_json::from_value(value).unwrap();
    assert_eq!(session.max_calls_in_request(), None);
}

/// A server that does not name the limit is answered with `None` rather than a
/// guess. RFC 8620 §2 requires the property, so this is a server out of spec —
/// and inventing a limit for it would refuse uploads it would have taken.
#[test]
fn a_session_that_names_no_upload_limit_says_so() {
    let mut value = fixture("core/session.json");
    value["capabilities"][jmap_proto::session::CAPABILITY_CORE]
        .as_object_mut()
        .unwrap()
        .remove("maxSizeUpload");
    let session: Session = serde_json::from_value(value).unwrap();
    assert_eq!(session.max_size_upload(), None);
}
