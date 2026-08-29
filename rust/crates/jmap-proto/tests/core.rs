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

/// RFC 8620 §2 explicitly permits a server to omit `primaryAccounts` outright
/// ("a server that does not support this concept MUST omit this property").
/// Deserializing such a session document must succeed with empty primary_accounts.
#[test]
fn session_deserializes_when_primary_accounts_is_omitted_by_server() {
    let session: Session = serde_json::from_value(serde_json::json!({
        "capabilities": {"urn:ietf:params:jmap:core": {}},
        "accounts": {
            "A1": {
                "name": "a@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {"urn:ietf:params:jmap:core": {}}
            }
        },
        "username": "a@example.com",
        "apiUrl": "https://jmap.example.com/api/",
        "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}/",
        "eventSourceUrl": "https://jmap.example.com/eventsource/",
        "state": "s1"
    }))
    .expect("session without primaryAccounts must deserialize cleanly");

    assert!(session.primary_accounts.is_empty());
}

/// RFC 8620 §5.5 defines `collation` on `Comparator` as an optional string.
#[test]
fn comparator_roundtrips_collation_when_specified() {
    let value = serde_json::json!({
        "property": "receivedAt",
        "isAscending": false,
        "collation": "i;unicode-casemap"
    });
    let comparator: Comparator = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(comparator.property, "receivedAt");
    assert!(!comparator.is_ascending);
    assert_eq!(comparator.collation.as_deref(), Some("i;unicode-casemap"));

    let round_tripped = serde_json::to_value(&comparator).unwrap();
    assert_eq!(round_tripped, value);
}

/// RFC 8620 §5.3 defines well-known `tooLarge` set error code.
#[test]
fn set_error_has_too_large_code() {
    assert_eq!(jmap_proto::error::set::TOO_LARGE, "tooLarge");
}

/// A `/get` response carrying unknown server extensions deserializes without error.
#[test]
fn get_response_ignores_unknown_server_properties() {
    let value = serde_json::json!({
        "accountId": "A1",
        "state": "s1",
        "list": [{"id": "1"}],
        "notFound": [],
        "customExtension": "ignored_cleanly"
    });
    let resp: jmap_proto::methods::GetResponse<serde_json::Value> =
        serde_json::from_value(value).expect("GetResponse must deserialize unknown fields cleanly");
    assert_eq!(resp.account_id.as_str(), "A1");
    assert_eq!(resp.state.as_str(), "s1");
    assert_eq!(resp.list.len(), 1);
}

/// A `/changes` response omitting optional empty arrays and false hasMoreChanges deserializes cleanly.
#[test]
fn changes_response_deserializes_with_omitted_empty_lists_and_defaults() {
    let value = serde_json::json!({
        "accountId": "A1",
        "oldState": "s1",
        "newState": "s2",
        "futureMember": 123
    });
    let resp: jmap_proto::methods::ChangesResponse = serde_json::from_value(value).unwrap();
    assert_eq!(resp.account_id.as_str(), "A1");
    assert_eq!(resp.old_state.as_str(), "s1");
    assert_eq!(resp.new_state.as_str(), "s2");
    assert!(!resp.has_more_changes);
    assert!(resp.created.is_empty());
    assert!(resp.updated.is_empty());
    assert!(resp.destroyed.is_empty());
}

/// A `/query` response omitting optional position/canCalculateChanges defaults cleanly.
#[test]
fn query_response_deserializes_with_omitted_defaults() {
    let value = serde_json::json!({
        "accountId": "A1",
        "queryState": "qs1",
        "extraMetric": "abc"
    });
    let resp: jmap_proto::methods::QueryResponse = serde_json::from_value(value).unwrap();
    assert_eq!(resp.account_id.as_str(), "A1");
    assert_eq!(resp.query_state.as_str(), "qs1");
    assert!(!resp.can_calculate_changes);
    assert_eq!(resp.position, 0);
    assert!(resp.ids.is_empty());
}

/// RFC 8620 §2 mandates: "eventSourceUrl: String ... A server that does not support this MUST omit this property."
/// Deserializing such a session document must succeed without error.
#[test]
fn session_deserializes_when_event_source_url_is_omitted_by_server() {
    let session: Session = serde_json::from_value(serde_json::json!({
        "capabilities": {"urn:ietf:params:jmap:core": {}},
        "accounts": {
            "A1": {
                "name": "a@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {"urn:ietf:params:jmap:core": {}}
            }
        },
        "username": "a@example.com",
        "apiUrl": "https://jmap.example.com/api/",
        "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}/",
        "state": "s1"
    }))
    .expect("session without eventSourceUrl must deserialize cleanly");

    assert_eq!(session.event_source_url, "");
}

/// A `/get` response omitting the `list` array deserializes cleanly with an empty list.
#[test]
fn get_response_deserializes_when_list_is_omitted_by_server() {
    let value = serde_json::json!({
        "accountId": "A1",
        "state": "s1",
        "notFound": ["id1"]
    });
    let resp: jmap_proto::methods::GetResponse<serde_json::Value> =
        serde_json::from_value(value).expect("GetResponse without list must deserialize cleanly");
    assert_eq!(resp.account_id.as_str(), "A1");
    assert_eq!(resp.state.as_str(), "s1");
    assert!(resp.list.is_empty());
    assert_eq!(resp.not_found.len(), 1);
}

/// A response envelope omitting `methodResponses` deserializes cleanly with an empty list.
#[test]
fn response_envelope_deserializes_when_method_responses_is_omitted_by_server() {
    let value = serde_json::json!({
        "sessionState": "s1"
    });
    let resp: jmap_proto::response::Response = serde_json::from_value(value)
        .expect("Response without methodResponses must deserialize cleanly");
    assert_eq!(resp.session_state.as_str(), "s1");
    assert!(resp.method_responses.is_empty());
}

/// Error codes and problem types in `jmap_proto::error` cover RFC 8620 specifications.
#[test]
fn core_error_constants_cover_rfc8620() {
    assert_eq!(
        jmap_proto::error::method::SERVER_UNAVAILABLE,
        "serverUnavailable"
    );
    assert_eq!(
        jmap_proto::error::method::SERVER_PARTIAL_FAIL,
        "serverPartialFail"
    );
    assert_eq!(
        jmap_proto::error::method::UNKNOWN_CAPABILITY,
        "unknownCapability"
    );

    assert_eq!(jmap_proto::error::set::RATE_LIMIT, "rateLimit");
    assert_eq!(jmap_proto::error::set::STATE_MISMATCH, "stateMismatch");
    assert_eq!(jmap_proto::error::set::REQUEST_TOO_LARGE, "requestTooLarge");

    assert_eq!(
        jmap_proto::error::request::UNKNOWN_CAPABILITY,
        "urn:ietf:params:jmap:error:unknownCapability"
    );
    assert_eq!(
        jmap_proto::error::request::NOT_JSON,
        "urn:ietf:params:jmap:error:notJSON"
    );
    assert_eq!(
        jmap_proto::error::request::NOT_REQUEST,
        "urn:ietf:params:jmap:error:notRequest"
    );
    assert_eq!(
        jmap_proto::error::request::LIMIT,
        "urn:ietf:params:jmap:error:limit"
    );
}

/// `Id` implements `Default`.
#[test]
fn id_implements_default_trait() {
    let id = Id::default();
    assert_eq!(id.as_str(), "");
}

/// `RequestError` supports standard RFC 7807 / RFC 8620 §3.6.1 problem details members.
#[test]
fn request_error_problem_details_fields_cover_rfc7807_and_rfc8620() {
    let value = serde_json::json!({
        "type": "urn:ietf:params:jmap:error:limit",
        "status": 400,
        "title": "Request limit exceeded",
        "detail": "maxCallsInRequest exceeded",
        "instance": "urn:uuid:12345678-1234-5678-1234-567812345678",
        "limit": "maxCallsInRequest"
    });
    let err: jmap_proto::error::RequestError = serde_json::from_value(value).unwrap();
    assert_eq!(err.error_type, "urn:ietf:params:jmap:error:limit");
    assert_eq!(err.status, Some(400));
    assert_eq!(
        err.extra.get("title"),
        Some(&serde_json::json!("Request limit exceeded"))
    );
    assert_eq!(err.detail.as_deref(), Some("maxCallsInRequest exceeded"));
    assert_eq!(
        err.extra.get("instance"),
        Some(&serde_json::json!(
            "urn:uuid:12345678-1234-5678-1234-567812345678"
        ))
    );
    assert_eq!(
        err.extra.get("limit"),
        Some(&serde_json::json!("maxCallsInRequest"))
    );
}

/// `QueryRequest` supports RFC 8620 §5.5 anchor and anchorOffset properties.
#[test]
fn query_request_anchor_and_anchor_offset_cover_rfc8620() {
    let req = QueryRequest::<serde_json::Value>::new("A1")
        .anchor("msg_123")
        .anchor_offset(-5);
    assert_eq!(req.anchor.as_ref().unwrap().as_str(), "msg_123");
    assert_eq!(req.anchor_offset, Some(-5));

    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["anchor"], "msg_123");
    assert_eq!(json["anchorOffset"], -5);

    let roundtrip: QueryRequest<serde_json::Value> = serde_json::from_value(json).unwrap();
    assert_eq!(roundtrip.anchor.as_ref().unwrap().as_str(), "msg_123");
    assert_eq!(roundtrip.anchor_offset, Some(-5));
}

/// `QueryChangesRequest` and `QueryChangesResponse` support RFC 8620 §5.6.
#[test]
fn query_changes_request_and_response_roundtrip_covers_rfc8620() {
    use jmap_proto::methods::{AddedItem, QueryChangesRequest, QueryChangesResponse};

    let req = QueryChangesRequest::<serde_json::Value>::new("A1", "qs_old")
        .filter(serde_json::json!({"hasKeyword": "$seen"}))
        .max_changes(50)
        .up_to_id("msg_99")
        .calculate_total();

    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["accountId"], "A1");
    assert_eq!(json["sinceQueryState"], "qs_old");
    assert_eq!(json["maxChanges"], 50);
    assert_eq!(json["upToId"], "msg_99");
    assert_eq!(json["calculateTotal"], true);

    let resp_val = serde_json::json!({
        "accountId": "A1",
        "oldQueryState": "qs_old",
        "newQueryState": "qs_new",
        "total": 12,
        "removed": ["id1", "id2"],
        "added": [
            {"id": "id3", "index": 0},
            {"id": "id4", "index": 5}
        ]
    });
    let resp: QueryChangesResponse = serde_json::from_value(resp_val.clone()).unwrap();
    assert_eq!(resp.account_id.as_str(), "A1");
    assert_eq!(resp.old_query_state.as_str(), "qs_old");
    assert_eq!(resp.new_query_state.as_str(), "qs_new");
    assert_eq!(resp.total, Some(12));
    assert_eq!(resp.removed.len(), 2);
    assert_eq!(resp.added.len(), 2);
    assert_eq!(resp.added[0], AddedItem::new("id3", 0));
    assert_eq!(resp.added[1], AddedItem::new("id4", 5));

    let round_tripped = serde_json::to_value(&resp).unwrap();
    assert_eq!(round_tripped, resp_val);
}

/// `CopyRequest` and `CopyResponse` support RFC 8620 §5.4.
#[test]
fn copy_request_and_response_roundtrip_covers_rfc8620() {
    use jmap_proto::methods::{CopyRequest, CopyResponse};

    let req = CopyRequest::<serde_json::Value>::new("SrcAccount", "DstAccount")
        .if_from_in_state("src_state_1")
        .if_in_state("dst_state_1")
        .copy_object("c1", serde_json::json!({"name": "Item 1"}))
        .on_success_destroy_original()
        .destroy_from_if_in_state("src_state_1");

    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["fromAccountId"], "SrcAccount");
    assert_eq!(json["ifFromInState"], "src_state_1");
    assert_eq!(json["accountId"], "DstAccount");
    assert_eq!(json["ifInState"], "dst_state_1");
    assert_eq!(json["onSuccessDestroyOriginal"], true);
    assert_eq!(json["destroyFromIfInState"], "src_state_1");
    assert_eq!(json["create"]["c1"]["name"], "Item 1");

    let resp_val = serde_json::json!({
        "fromAccountId": "SrcAccount",
        "accountId": "DstAccount",
        "oldState": "s1",
        "newState": "s2",
        "created": {
            "c1": {"id": "dst_id_1"}
        }
    });
    let resp: CopyResponse<serde_json::Value> = serde_json::from_value(resp_val.clone()).unwrap();
    assert_eq!(resp.from_account_id.as_str(), "SrcAccount");
    assert_eq!(resp.account_id.as_str(), "DstAccount");
    assert_eq!(resp.old_state.as_ref().unwrap().as_str(), "s1");
    assert_eq!(resp.new_state.as_str(), "s2");
    assert!(resp.created.unwrap().contains_key("c1"));
}

/// Standard filter operators and error codes in RFC 8620 are defined.
#[test]
fn standard_filter_operators_and_error_codes_cover_rfc8620() {
    assert_eq!(jmap_proto::methods::filter_operator::AND, "AND");
    assert_eq!(jmap_proto::methods::filter_operator::OR, "OR");
    assert_eq!(jmap_proto::methods::filter_operator::NOT, "NOT");

    assert_eq!(jmap_proto::error::set::ALREADY_EXISTS, "alreadyExists");
    assert_eq!(
        jmap_proto::error::method::FROM_STATE_MISMATCH,
        "fromStateMismatch"
    );
    assert_eq!(
        jmap_proto::error::set::CANNOT_DESTROY_ORIGINAL,
        "cannotDestroyOriginal"
    );
}

/// PushSubscription, PushVerification, and StateChange types cover RFC 8620 §7.
#[test]
fn push_and_state_change_roundtrip_covers_rfc8620() {
    use jmap_proto::push::{
        PushSubscription, PushSubscriptionKeys, PushVerification, StateChange,
        push_subscription_set_error,
    };
    use std::collections::BTreeMap;

    assert_eq!(push_subscription_set_error::INVALID_URL, "invalidUrl");
    assert_eq!(
        push_subscription_set_error::EXPIRES_TOO_FAR,
        "expiresTooFar"
    );

    let sub = PushSubscription {
        id: Some(Id::new("sub_1")),
        device_client_id: "device_abc".to_owned(),
        url: "https://push.example.com/endpoint".to_owned(),
        keys: Some(PushSubscriptionKeys {
            p256dh: "key_p256dh".to_owned(),
            auth: "key_auth".to_owned(),
        }),
        expires: Some(UtcDate::new("2026-10-01T00:00:00Z")),
        types: Some(vec!["Email".to_owned(), "CalendarEvent".to_owned()]),
        extra: BTreeMap::new(),
    };

    let sub_val = serde_json::to_value(&sub).unwrap();
    assert_eq!(sub_val["id"], "sub_1");
    assert_eq!(sub_val["deviceClientId"], "device_abc");
    assert_eq!(sub_val["url"], "https://push.example.com/endpoint");
    assert_eq!(sub_val["keys"]["p256dh"], "key_p256dh");
    assert_eq!(sub_val["expires"], "2026-10-01T00:00:00Z");
    assert_eq!(
        sub_val["types"],
        serde_json::json!(["Email", "CalendarEvent"])
    );

    let round_tripped: PushSubscription = serde_json::from_value(sub_val).unwrap();
    assert_eq!(round_tripped, sub);

    let verification = PushVerification {
        object_type: "PushVerification".to_owned(),
        push_subscription_id: Id::new("sub_1"),
        verification_code: "code_12345".to_owned(),
        extra: BTreeMap::new(),
    };
    let ver_val = serde_json::to_value(&verification).unwrap();
    assert_eq!(ver_val["@type"], "PushVerification");
    assert_eq!(ver_val["pushSubscriptionId"], "sub_1");
    assert_eq!(ver_val["verificationCode"], "code_12345");

    let ver_round_tripped: PushVerification = serde_json::from_value(ver_val).unwrap();
    assert_eq!(ver_round_tripped, verification);

    let state_change = StateChange {
        object_type: "StateChange".to_owned(),
        changed: BTreeMap::from([(
            Id::new("A1"),
            BTreeMap::from([
                ("Email".to_owned(), State::new("s_email_1")),
                ("Mailbox".to_owned(), State::new("s_box_1")),
            ]),
        )]),
        extra: BTreeMap::new(),
    };
    let sc_val = serde_json::to_value(&state_change).unwrap();
    assert_eq!(sc_val["@type"], "StateChange");
    assert_eq!(sc_val["changed"]["A1"]["Email"], "s_email_1");

    let sc_round_tripped: StateChange = serde_json::from_value(sc_val).unwrap();
    assert_eq!(sc_round_tripped, state_change);
}

/// BlobCopyRequest, BlobCopyResponse, and CoreCapability cover RFC 8620 §5.7 & §2.
#[test]
fn blob_copy_and_core_capabilities_roundtrip_covers_rfc8620() {
    use jmap_proto::methods::{BlobCopyRequest, BlobCopyResponse};
    use jmap_proto::session::CoreCapability;
    use std::collections::BTreeMap;

    let req = BlobCopyRequest::new("acc_source", "acc_target", vec!["blob_1", "blob_2"]);
    let req_val = serde_json::to_value(&req).unwrap();
    assert_eq!(req_val["fromAccountId"], "acc_source");
    assert_eq!(req_val["accountId"], "acc_target");
    assert_eq!(req_val["blobIds"], serde_json::json!(["blob_1", "blob_2"]));

    let round_req: BlobCopyRequest = serde_json::from_value(req_val).unwrap();
    assert_eq!(round_req, req);

    let resp = BlobCopyResponse {
        from_account_id: Id::new("acc_source"),
        account_id: Id::new("acc_target"),
        copied: Some(BTreeMap::from([(Id::new("blob_1"), Id::new("blob_1_new"))])),
        not_copied: Some(BTreeMap::from([(
            Id::new("blob_2"),
            jmap_proto::error::SetError::new("notFound"),
        )])),
    };
    let resp_val = serde_json::to_value(&resp).unwrap();
    assert_eq!(resp_val["copied"]["blob_1"], "blob_1_new");
    assert_eq!(resp_val["notCopied"]["blob_2"]["type"], "notFound");

    let round_resp: BlobCopyResponse = serde_json::from_value(resp_val).unwrap();
    assert_eq!(round_resp, resp);

    let cap = CoreCapability {
        max_size_upload: 10_000_000,
        max_concurrent_upload: 4,
        max_size_request: 5_000_000,
        max_concurrent_requests: 8,
        max_calls_in_request: 16,
        max_objects_in_get: 256,
        max_objects_in_set: 128,
        collation_algorithms: vec!["i;ascii-casemap".to_owned(), "i;octet".to_owned()],
        extra: BTreeMap::new(),
    };
    let cap_val = serde_json::to_value(&cap).unwrap();
    assert_eq!(cap_val["maxSizeUpload"], 10_000_000);
    assert_eq!(cap_val["maxConcurrentUpload"], 4);
    assert_eq!(cap_val["maxSizeRequest"], 5_000_000);
    assert_eq!(cap_val["maxConcurrentRequests"], 8);
    assert_eq!(cap_val["maxCallsInRequest"], 16);
    assert_eq!(cap_val["maxObjectsInGet"], 256);
    assert_eq!(cap_val["maxObjectsInSet"], 128);
    assert_eq!(
        cap_val["collationAlgorithms"],
        serde_json::json!(["i;ascii-casemap", "i;octet"])
    );

    let round_cap: CoreCapability = serde_json::from_value(cap_val.clone()).unwrap();
    assert_eq!(round_cap, cap);

    let session: Session = serde_json::from_value(serde_json::json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": cap_val
        },
        "accounts": {},
        "username": "user@example.com",
        "apiUrl": "https://api.example.com/jmap",
        "downloadUrl": "https://api.example.com/download",
        "uploadUrl": "https://api.example.com/upload",
        "state": "s1"
    }))
    .unwrap();

    assert_eq!(session.max_concurrent_upload(), Some(4));
    assert_eq!(session.max_concurrent_requests(), Some(8));
    assert_eq!(session.max_objects_in_set(), Some(128));
    assert_eq!(
        session.collation_algorithms(),
        Some(vec!["i;ascii-casemap".to_owned(), "i;octet".to_owned()])
    );
    assert_eq!(session.core_capability(), Some(cap));
}

#[test]
fn query_request_and_changes_request_calculate_total_builder() {
    use jmap_proto::methods::{QueryChangesRequest, QueryRequest};

    let q: QueryRequest<()> = QueryRequest::new("A1").calculate_total();
    assert!(q.calculate_total);
    let q_val = serde_json::to_value(&q).unwrap();
    assert_eq!(q_val["calculateTotal"], true);

    let qc: QueryChangesRequest<()> = QueryChangesRequest::new("A1", "s1").calculate_total();
    assert!(qc.calculate_total);
    let qc_val = serde_json::to_value(&qc).unwrap();
    assert_eq!(qc_val["calculateTotal"], true);
}

#[test]
fn query_request_and_changes_request_collapse_threads_builder() {
    use jmap_proto::methods::{QueryChangesRequest, QueryRequest};

    let q: QueryRequest<()> = QueryRequest::new("A1").collapse_threads();
    assert!(q.collapse_threads);
    let q_val = serde_json::to_value(&q).unwrap();
    assert_eq!(q_val["collapseThreads"], true);

    let qc: QueryChangesRequest<()> = QueryChangesRequest::new("A1", "s1").collapse_threads();
    assert!(qc.collapse_threads);
    let qc_val = serde_json::to_value(&qc).unwrap();
    assert_eq!(qc_val["collapseThreads"], true);
}

#[test]
fn response_deserializes_missing_session_state_with_default() {
    use jmap_proto::response::Response;

    let value = serde_json::json!({
        "methodResponses": []
    });

    let resp: Response = serde_json::from_value(value).unwrap();
    assert_eq!(resp.session_state.as_str(), "");
}

#[test]
fn result_reference_and_push_builders_roundtrip() {
    use jmap_proto::UtcDate;
    use jmap_proto::push::{PushSubscription, PushVerification, StateChange};
    use jmap_proto::request::ResultReference;
    use std::collections::BTreeMap;

    let rr = ResultReference::new("call1", "Email/query", "/ids");
    assert_eq!(rr.result_of, "call1");
    assert_eq!(rr.name, "Email/query");
    assert_eq!(rr.path, "/ids");
    let rr_val = serde_json::to_value(&rr).unwrap();
    assert_eq!(rr_val["resultOf"], "call1");
    assert_eq!(rr_val["name"], "Email/query");
    assert_eq!(rr_val["path"], "/ids");
    assert_eq!(
        serde_json::from_value::<ResultReference>(rr_val).unwrap(),
        rr
    );

    let sub = PushSubscription::new("dev_1", "https://push.example.com/sub")
        .with_keys("key_p256", "auth_secret")
        .with_expires(UtcDate::new("2026-10-01T00:00:00Z"))
        .with_types(["Email", "ContactCard", "CalendarEvent"]);

    assert_eq!(sub.device_client_id, "dev_1");
    assert_eq!(sub.url, "https://push.example.com/sub");
    assert_eq!(sub.keys.as_ref().unwrap().p256dh, "key_p256");
    assert_eq!(sub.keys.as_ref().unwrap().auth, "auth_secret");
    assert_eq!(
        sub.expires.as_ref().unwrap().as_str(),
        "2026-10-01T00:00:00Z"
    );
    assert_eq!(sub.types.as_ref().unwrap().len(), 3);

    let ver = PushVerification::new("sub_42", "challenge_xyz");
    assert_eq!(ver.object_type, "PushVerification");
    assert_eq!(ver.push_subscription_id.as_str(), "sub_42");
    assert_eq!(ver.verification_code, "challenge_xyz");

    let change = StateChange::new(BTreeMap::from([(
        "acc1".into(),
        BTreeMap::from([("Email".to_owned(), "state_123".into())]),
    )]));
    assert_eq!(change.object_type, "StateChange");
    assert_eq!(
        change.changed[&"acc1".into()]["Email"].as_str(),
        "state_123"
    );
}

#[test]
fn core_echo_and_builder_methods_roundtrip() {
    use jmap_proto::methods::{ChangesRequest, Echo, GetRequest, UploadResponse};
    use jmap_proto::request::{Invocation, Request, ResultReference};
    use jmap_proto::response::Response;
    use std::collections::BTreeMap;

    // Core/echo
    let echo = Echo::new(serde_json::json!({"testKey": "testVal", "num": 42}));
    let echo_val = serde_json::to_value(&echo).unwrap();
    assert_eq!(echo_val["testKey"], "testVal");
    assert_eq!(echo_val["num"], 42);
    let round_echo: Echo<serde_json::Value> = serde_json::from_value(echo_val).unwrap();
    assert_eq!(round_echo, echo);

    // GetRequest builders
    let rr = ResultReference::new("call0", "Email/query", "/ids");
    let get_req = GetRequest::all("acc1")
        .properties(["id", "blobId", "threadId"])
        .ids_ref(rr);
    assert_eq!(
        get_req.properties.as_ref().unwrap(),
        &["id", "blobId", "threadId"]
    );
    assert_eq!(get_req.ids_ref.as_ref().unwrap().result_of, "call0");

    // ChangesRequest builders
    let chg_req = ChangesRequest::new("acc1", "s1").max_changes(50);
    assert_eq!(chg_req.account_id.as_str(), "acc1");
    assert_eq!(chg_req.since_state.as_str(), "s1");
    assert_eq!(chg_req.max_changes, Some(50));

    // UploadResponse builders
    let upload = UploadResponse::new("acc1", "blob_99", 2048).with_content_type("text/plain");
    assert_eq!(upload.account_id.as_str(), "acc1");
    assert_eq!(upload.blob_id.as_str(), "blob_99");
    assert_eq!(upload.size, 2048);
    assert_eq!(upload.content_type.as_deref(), Some("text/plain"));

    // Request with_created_ids
    let req = Request::new(["urn:ietf:params:jmap:core"])
        .with_created_ids(BTreeMap::from([("c1".into(), "id1".into())]));
    assert_eq!(
        req.created_ids.as_ref().unwrap()[&"c1".into()].as_str(),
        "id1"
    );

    // Response builders and Invocation from_value
    let inv = Invocation::from_value("Core/echo", serde_json::json!({"ack": true}), "c0");
    let resp = Response::new("s_final")
        .with_method_response(inv)
        .with_created_ids(BTreeMap::from([("c1".into(), "id1".into())]));
    assert_eq!(resp.session_state.as_str(), "s_final");
    assert_eq!(resp.method_responses.len(), 1);
    assert_eq!(resp.method_responses[0].name, "Core/echo");
    assert_eq!(
        resp.created_ids.as_ref().unwrap()[&"c1".into()].as_str(),
        "id1"
    );
}

#[test]
fn session_typed_capability_accessors_and_websocket_roundtrip() {
    use jmap_proto::session::{
        CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
        CAPABILITY_MDN, CAPABILITY_PRINCIPALS, CAPABILITY_SUBMISSION, CAPABILITY_VACATION_RESPONSE,
        CAPABILITY_WEBSOCKET, WebSocketCapability,
    };

    let session: Session = serde_json::from_value(serde_json::json!({
        "capabilities": {
            CAPABILITY_CORE: {
                "maxSizeUpload": 10000000,
                "maxConcurrentUpload": 4,
                "maxSizeRequest": 5000000,
                "maxConcurrentRequests": 8,
                "maxCallsInRequest": 16,
                "maxObjectsInGet": 500,
                "maxObjectsInSet": 500,
                "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
            },
            CAPABILITY_MAIL: {
                "maxSizeAttachmentsPerEmail": 50000000,
                "maxSizeEmailInBytes": 100000000,
                "mayCreateTopLevelMailbox": true
            },
            CAPABILITY_SUBMISSION: {
                "maxDelayedSend": 86400,
                "submissionExtensions": {"futurerelease": []}
            },
            CAPABILITY_CONTACTS: {
                "maxSizeAttachmentsPerCard": 5000000,
                "maxNumberOfCardsInSet": 100
            },
            CAPABILITY_CALENDARS: {
                "maxSizeAttachmentsPerEvent": 20000000,
                "maxConcurrentAvailabilities": 10
            },
            CAPABILITY_PRINCIPALS: {
                "maxPrincipalsPerGet": 50
            },
            CAPABILITY_WEBSOCKET: {
                "url": "wss://jmap.example.com/ws",
                "supportsPush": true
            },
            CAPABILITY_VACATION_RESPONSE: {},
            CAPABILITY_MDN: {}
        },
        "accounts": {
            "A1": {
                "name": "john@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    CAPABILITY_CORE: {},
                    CAPABILITY_MAIL: {}
                }
            }
        },
        "primaryAccounts": {
            CAPABILITY_MAIL: "A1"
        },
        "username": "john@example.com",
        "apiUrl": "https://jmap.example.com/api",
        "downloadUrl": "https://jmap.example.com/download/{blobId}",
        "uploadUrl": "https://jmap.example.com/upload/{accountId}",
        "eventSourceUrl": "https://jmap.example.com/events",
        "state": "s1234"
    }))
    .unwrap();

    let core_cap = session.core_capability().expect("core capability");
    assert_eq!(core_cap.max_size_upload, 10000000);
    assert_eq!(core_cap.max_calls_in_request, 16);

    let mail_cap = session.mail_capability().expect("mail capability");
    assert_eq!(mail_cap.max_size_attachments_per_email, 50000000);
    assert!(mail_cap.may_create_top_level_mailbox);

    let sub_cap = session
        .submission_capability()
        .expect("submission capability");
    assert_eq!(sub_cap.max_delayed_send, 86400);

    let contacts_cap = session.contacts_capability().expect("contacts capability");
    assert_eq!(contacts_cap.max_number_of_cards_in_set, 100);

    let cal_cap = session
        .calendars_capability()
        .expect("calendars capability");
    assert_eq!(cal_cap.max_concurrent_availabilities, 10);

    let princ_cap = session
        .principals_capability()
        .expect("principals capability");
    assert_eq!(princ_cap.max_principals_per_get, Some(50));

    let ws_cap = session
        .websocket_capability()
        .expect("websocket capability");
    assert_eq!(ws_cap.url, "wss://jmap.example.com/ws");
    assert!(ws_cap.supports_push);

    let ws_val = serde_json::to_value(&ws_cap).unwrap();
    assert_eq!(ws_val["url"], "wss://jmap.example.com/ws");
    assert!(ws_val["supportsPush"].as_bool().unwrap());
    assert_eq!(
        serde_json::from_value::<WebSocketCapability>(ws_val).unwrap(),
        ws_cap
    );
}

#[test]
fn core_response_and_request_error_builders() {
    use jmap_proto::error::{RequestError, SetError, request};
    use jmap_proto::methods::{
        AddedItem, BlobCopyResponse, ChangesResponse, CopyResponse, GetResponse,
        QueryChangesResponse, QueryResponse,
    };
    use std::collections::BTreeMap;

    let get_resp = GetResponse::new("acc1", "s1", vec![10, 20, 30]).with_not_found(["id4", "id5"]);
    assert_eq!(get_resp.account_id.as_str(), "acc1");
    assert_eq!(get_resp.state.as_str(), "s1");
    assert_eq!(get_resp.list, vec![10, 20, 30]);
    assert_eq!(get_resp.not_found.len(), 2);

    let changes_resp = ChangesResponse::new("acc1", "s1", "s2")
        .with_created(["c1"])
        .with_updated(["u1", "u2"])
        .with_destroyed(["d1"])
        .has_more_changes(true);
    assert_eq!(changes_resp.account_id.as_str(), "acc1");
    assert_eq!(changes_resp.old_state.as_str(), "s1");
    assert_eq!(changes_resp.new_state.as_str(), "s2");
    assert_eq!(changes_resp.created.len(), 1);
    assert_eq!(changes_resp.updated.len(), 2);
    assert_eq!(changes_resp.destroyed.len(), 1);
    assert!(changes_resp.has_more_changes);

    let query_resp = QueryResponse::new("acc1", "qs1", ["i1", "i2"])
        .with_total(100)
        .with_limit(2)
        .with_position(0)
        .can_calculate_changes(true);
    assert_eq!(query_resp.account_id.as_str(), "acc1");
    assert_eq!(query_resp.query_state.as_str(), "qs1");
    assert_eq!(query_resp.ids.len(), 2);
    assert_eq!(query_resp.total, Some(100));
    assert_eq!(query_resp.limit, Some(2));
    assert_eq!(query_resp.position, 0);
    assert!(query_resp.can_calculate_changes);

    let query_changes_resp = QueryChangesResponse::new("acc1", "qs1", "qs2")
        .with_added([AddedItem::new("i3", 2)])
        .with_removed(["i1"])
        .with_total(100);
    assert_eq!(query_changes_resp.account_id.as_str(), "acc1");
    assert_eq!(query_changes_resp.old_query_state.as_str(), "qs1");
    assert_eq!(query_changes_resp.new_query_state.as_str(), "qs2");
    assert_eq!(query_changes_resp.added.len(), 1);
    assert_eq!(query_changes_resp.removed.len(), 1);
    assert_eq!(query_changes_resp.total, Some(100));

    let blob_copy_resp = BlobCopyResponse::new("acc_src", "acc_dst")
        .with_copied(BTreeMap::from([("b1".into(), "b1_dst".into())]))
        .with_not_copied(BTreeMap::from([("b2".into(), SetError::new("notFound"))]));
    assert_eq!(blob_copy_resp.from_account_id.as_str(), "acc_src");
    assert_eq!(blob_copy_resp.account_id.as_str(), "acc_dst");
    assert_eq!(blob_copy_resp.copied.as_ref().unwrap().len(), 1);
    assert_eq!(blob_copy_resp.not_copied.as_ref().unwrap().len(), 1);

    let copy_resp = CopyResponse::<serde_json::Value>::new("acc_src", "acc_dst", "s2")
        .with_old_state("s1")
        .with_created(BTreeMap::from([(
            "k1".to_string(),
            serde_json::json!({"id": "new_1"}),
        )]))
        .with_not_created(BTreeMap::from([(
            "k2".to_string(),
            SetError::new("alreadyExists"),
        )]));
    assert_eq!(copy_resp.from_account_id.as_str(), "acc_src");
    assert_eq!(copy_resp.account_id.as_str(), "acc_dst");
    assert_eq!(copy_resp.old_state.as_ref().unwrap().as_str(), "s1");
    assert_eq!(copy_resp.new_state.as_str(), "s2");
    assert_eq!(copy_resp.created.as_ref().unwrap().len(), 1);
    assert_eq!(copy_resp.not_created.as_ref().unwrap().len(), 1);

    let req_err = RequestError::new(request::NOT_REQUEST)
        .with_status(400)
        .with_detail("The request was malformed JSON");
    assert_eq!(req_err.error_type, request::NOT_REQUEST);
    assert_eq!(req_err.status, Some(400));
    assert_eq!(
        req_err.detail.as_deref(),
        Some("The request was malformed JSON")
    );
}
