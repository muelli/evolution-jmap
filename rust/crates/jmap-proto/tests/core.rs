// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Round-trip tests for the RFC 8620 core types against fixture JSON taken
//! from the RFC examples. Comparison happens on `serde_json::Value` so a
//! field silently dropped or renamed by our types fails the test.

use jmap_proto::error::{MethodError, RequestError};
use jmap_proto::id::Id;
use jmap_proto::methods::{Comparator, GetRequest, QueryRequest};
use jmap_proto::request::{Request, ResultReference};
use jmap_proto::response::Response;
use jmap_proto::session::Session;
use jmap_proto::state::{State, UtcDate};
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
    assert!(!response.method_responses[0].is_error());
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

/// How many octets one request to `apiUrl` may carry (RFC 8620 §2), which is
/// what decides whether an `Email/get` naming a long list of ids may be sent as
/// it stands or has to be sent as several.
#[test]
fn the_session_names_the_largest_request_the_server_takes() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(session.max_size_request(), Some(10_000_000));
}

/// A server that names no request-size limit is answered with `None`, like the
/// other three: inventing a number here would split requests the server would
/// have taken whole, and the split ones are the ones with a window in them.
#[test]
fn a_session_that_names_no_request_size_limit_says_so() {
    let mut value = fixture("core/session.json");
    value["capabilities"][jmap_proto::session::CAPABILITY_CORE]
        .as_object_mut()
        .unwrap()
        .remove("maxSizeRequest");
    let session: Session = serde_json::from_value(value).unwrap();
    assert_eq!(session.max_size_request(), None);
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

/// How many ids one `/get` call may name (RFC 8620 §2), like the other three
/// core-capability limits above.
#[test]
fn the_session_names_how_many_ids_one_get_call_may_name() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(session.max_objects_in_get(), Some(256));
}

/// A server that names no `/get` limit is answered with `None`, like the
/// other three: a caller has to know it has no number to plan a batch size
/// around, not be handed an invented one.
#[test]
fn a_session_that_names_no_get_limit_says_so() {
    let mut value = fixture("core/session.json");
    value["capabilities"][jmap_proto::session::CAPABILITY_CORE]
        .as_object_mut()
        .unwrap()
        .remove("maxObjectsInGet");
    let session: Session = serde_json::from_value(value).unwrap();
    assert_eq!(session.max_objects_in_get(), None);
}

/// `Response::responses_for` groups a call id's (possibly several)
/// responses, in wire order, and nothing else's.
#[test]
fn responses_for_groups_by_call_id_in_order() {
    let response: Response =
        serde_json::from_value(fixture("core/response_with_error.json")).unwrap();

    let c2: Vec<&str> = response
        .responses_for("c2")
        .map(|invocation| invocation.name.as_str())
        .collect();
    assert_eq!(c2, vec!["method2", "anotherResponseFromMethod2"]);

    assert_eq!(response.responses_for("no-such-call").count(), 0);
}

#[test]
fn id_as_ref_borrows_the_inner_string() {
    let id = Id::from("A13824");
    assert_eq!(AsRef::<str>::as_ref(&id), "A13824");
}

#[test]
fn id_display_agrees_with_the_wire_value() {
    assert_eq!(Id::from("A13824").to_string(), "A13824");
}

#[test]
fn state_as_str_and_display_agree_with_the_wire_value() {
    let state = State::from("75128aab4b1b");
    assert_eq!(state.as_str(), "75128aab4b1b");
    assert_eq!(state.to_string(), "75128aab4b1b");
}

#[test]
fn utc_date_as_str_and_display_agree_with_the_wire_value() {
    let date = UtcDate::from("2026-08-19T06:00:00Z");
    assert_eq!(date.as_str(), "2026-08-19T06:00:00Z");
    assert_eq!(date.to_string(), "2026-08-19T06:00:00Z");
}

/// `primary_account` reads `primaryAccounts` verbatim: present for a
/// capability the server named one for, `None` for one it didn't.
#[test]
fn primary_account_reads_the_map_verbatim() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(
        session.primary_account(jmap_proto::session::CAPABILITY_MAIL),
        Some(&Id::from("A13824"))
    );
    assert_eq!(
        session.primary_account("urn:ietf:params:jmap:vacationresponse"),
        None
    );
}

/// `resolve_primary_account` believes a `primaryAccounts` entry once the
/// named account actually claims the capability.
#[test]
fn resolve_primary_account_trusts_a_consistent_primary_accounts_entry() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(
        session.resolve_primary_account(jmap_proto::session::CAPABILITY_MAIL),
        Some(&Id::from("A13824"))
    );
}

/// A capability the session never mentions at all resolves to `None` — a
/// `using` naming it would be answered `unknownCapability`, so nothing
/// behind it is reachable.
#[test]
fn resolve_primary_account_is_none_for_an_unnamed_capability() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(
        session.resolve_primary_account("urn:ietf:params:jmap:vacationresponse"),
        None
    );
}

/// The fixture's `primaryAccounts` names "A13824" for `contacts`, but that
/// account's own `accountCapabilities` never claims `contacts` — a
/// contradiction in the document per the doc comment, and not believed.
#[test]
fn resolve_primary_account_rejects_a_primary_accounts_entry_the_account_does_not_claim() {
    let session: Session = serde_json::from_value(fixture("core/session.json")).unwrap();
    assert_eq!(
        session.resolve_primary_account(jmap_proto::session::CAPABILITY_CONTACTS),
        None
    );
}

/// With no `primaryAccounts` entry at all, the one personal account that
/// claims the capability is inferred.
#[test]
fn resolve_primary_account_falls_back_to_the_sole_personal_account() {
    let session: Session = serde_json::from_value(serde_json::json!({
        "capabilities": {"urn:ietf:params:jmap:mail": {}},
        "accounts": {
            "A1": {
                "name": "a@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {"urn:ietf:params:jmap:mail": {}}
            }
        },
        "primaryAccounts": {},
        "username": "a@example.com",
        "apiUrl": "https://jmap.example.com/api/",
        "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}/",
        "eventSourceUrl": "https://jmap.example.com/eventsource/",
        "state": "s1"
    }))
    .unwrap();
    assert_eq!(
        session.resolve_primary_account(jmap_proto::session::CAPABILITY_MAIL),
        Some(&Id::from("A1"))
    );
}

/// The fallback requires *both* personal and capability-claiming — an
/// account that is personal but does not claim the capability must not be
/// inferred just because it is the only account around.
#[test]
fn resolve_primary_account_fallback_requires_personal_and_the_capability_together() {
    let session: Session = serde_json::from_value(serde_json::json!({
        "capabilities": {"urn:ietf:params:jmap:mail": {}},
        "accounts": {
            "A1": {
                "name": "a@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {}
            }
        },
        "primaryAccounts": {},
        "username": "a@example.com",
        "apiUrl": "https://jmap.example.com/api/",
        "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}/",
        "eventSourceUrl": "https://jmap.example.com/eventsource/",
        "state": "s1"
    }))
    .unwrap();
    assert_eq!(
        session.resolve_primary_account(jmap_proto::session::CAPABILITY_MAIL),
        None
    );
}

/// `GetRequest::ids` actually sets `ids`, not just the fields `all` already
/// sets.
#[test]
fn get_request_ids_builder_sets_the_ids_field() {
    let request = GetRequest::ids(Id::from("A1"), [Id::from("M1"), Id::from("M2")]);
    assert_eq!(request.ids, Some(vec![Id::from("M1"), Id::from("M2")]));
}

/// `position: 0` and `calculateTotal: false` are `/query`'s defaults (RFC
/// 8620 §5.5) and are omitted from the wire, not sent as explicit zeroes.
#[test]
fn query_request_omits_default_position_and_calculate_total() {
    let request = QueryRequest::<Value>::new(Id::from("A1"));
    let value = serde_json::to_value(&request).unwrap();
    assert!(value.get("position").is_none());
    assert!(value.get("calculateTotal").is_none());
}

/// A non-default position or an explicit `calculateTotal: true` must survive
/// onto the wire — the omission above is specifically for the defaults, not
/// for these fields generally.
#[test]
fn query_request_keeps_a_nonzero_position_and_true_calculate_total() {
    let mut request = QueryRequest::<Value>::new(Id::from("A1"));
    request.position = 5;
    request.calculate_total = true;
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["position"], serde_json::json!(5));
    assert_eq!(value["calculateTotal"], serde_json::json!(true));
}

/// A `Comparator` the wire omits `isAscending` on defaults to ascending (RFC
/// 8620 §5.5).
#[test]
fn comparator_defaults_to_ascending_when_the_wire_omits_it() {
    let comparator: Comparator =
        serde_json::from_value(serde_json::json!({"property": "foo"})).unwrap();
    assert!(comparator.is_ascending);
}

/// The fallback also excludes an account that claims the capability but is
/// not personal (a shared account) — `isPersonal` is one of the two
/// conditions the fallback requires, not a redundant one alongside the
/// capability check `resolve_primary_account` does afterwards.
#[test]
fn resolve_primary_account_fallback_excludes_a_non_personal_account() {
    let session: Session = serde_json::from_value(serde_json::json!({
        "capabilities": {"urn:ietf:params:jmap:mail": {}},
        "accounts": {
            "A1": {
                "name": "shared@example.com",
                "isPersonal": false,
                "isReadOnly": false,
                "accountCapabilities": {"urn:ietf:params:jmap:mail": {}}
            }
        },
        "primaryAccounts": {},
        "username": "a@example.com",
        "apiUrl": "https://jmap.example.com/api/",
        "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}/",
        "eventSourceUrl": "https://jmap.example.com/eventsource/",
        "state": "s1"
    }))
    .unwrap();
    assert_eq!(
        session.resolve_primary_account(jmap_proto::session::CAPABILITY_MAIL),
        None
    );
}
