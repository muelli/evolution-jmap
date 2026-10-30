// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dispatch JMAP API calls, resolve result references (RFC 8620 §3.7), and route methods.

use jmap_proto::Id;
use jmap_proto::error::{self, MethodError};
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use serde_json::{Map, Value};

use crate::state::{ServerState, TypeStateSnapshot};

/// Handle one JMAP API POST.
/// Return an HTTP status code and JSON body.
/// Use `caller` as the authenticated principal. `None` means anonymous access.
pub fn handle_api(state: &mut ServerState, body: &[u8], caller: Option<&Id>) -> (u16, Value) {
    if let Ok(text) = std::str::from_utf8(body) {
        println!("--> POST /jmap\n{text}");
    }

    // Count each HTTP request, including requests rejected before parsing.
    state.api_requests += 1;

    // RFC 8620 §2: reject requests over `maxSizeRequest` before parsing method calls.
    if let Some(limit) = state.size_request
        && body.len() as u64 > limit
    {
        let problem = serde_json::json!({
            "type": "urn:ietf:params:jmap:error:limit",
            "limit": "maxSizeRequest",
            "status": 400,
            "detail": format!(
                "{} octets is more than the {limit} this server takes in one request",
                body.len(),
            ),
        });
        return (400, problem);
    }

    let request: Request = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(parse_error) => {
            let problem = serde_json::json!({
                "type": "urn:ietf:params:jmap:error:notRequest",
                "status": 400,
                "detail": format!("not a valid JMAP request: {parse_error}"),
            });
            return (400, problem);
        }
    };

    // RFC 8620 §3.2: reject the whole request when it exceeds `maxCallsInRequest`.
    if let Some(limit) = state.calls_in_request
        && request.method_calls.len() as u64 > limit
    {
        let problem = serde_json::json!({
            "type": "urn:ietf:params:jmap:error:limit",
            "limit": "maxCallsInRequest",
            "status": 400,
            "detail": format!(
                "{} method calls is more than the {limit} this server takes in one request",
                request.method_calls.len(),
            ),
        });
        return (400, problem);
    }

    let mut responses: Vec<Invocation> = Vec::new();
    // Track ids for later `/set` calls that resolve `#creationId` (RFC 8620 §5.3).
    let mut created_ids: std::collections::BTreeMap<String, jmap_proto::Id> =
        std::collections::BTreeMap::new();
    // Snapshot type states before dispatch to emit one `StateChange` per request.
    // This follows RFC 8620 §7.1 for batched mutations.
    let type_states_before = state.type_state_snapshot();
    for call in &request.method_calls {
        // Record each attempted method call, even when handling fails later.
        state.method_calls.push(call.name.clone());
        let invocation = match resolve_references(call, &responses) {
            Ok(arguments) => {
                match handle_method(state, &call.name, arguments, &created_ids, caller) {
                    Ok(result) => {
                        record_created_ids(&call.name, &result, &mut created_ids);
                        Invocation::from_value(call.name.clone(), result, call.call_id.clone())
                    }
                    Err(method_error) => error_invocation(method_error, &call.call_id),
                }
            }
            Err(method_error) => error_invocation(method_error, &call.call_id),
        };
        responses.push(invocation);
    }

    if let Some(change) = state_change_since(&type_states_before, &state.type_state_snapshot()) {
        state.event_source.broadcast(&change);
    }

    let response = Response::new(state.session_state()).with_method_responses(responses);

    let response_value = serde_json::to_value(&response).expect("response serializes");
    println!(
        "<-- 200 OK\n{}\n",
        serde_json::to_string_pretty(&response_value).unwrap_or_default()
    );

    (200, response_value)
}

fn error_invocation(error: MethodError, call_id: &str) -> Invocation {
    Invocation::from_value(
        "error",
        serde_json::to_value(&error).expect("error serializes"),
        call_id,
    )
}

/// Resolve `#argument` result references from earlier responses in this request.
/// Follow RFC 8620 §3.7.
fn resolve_references(call: &Invocation, responses: &[Invocation]) -> Result<Value, MethodError> {
    let Value::Object(arguments) = &call.arguments else {
        return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
            .with_description("method arguments must be an object"));
    };

    let mut resolved = Map::new();
    for (key, value) in arguments {
        if let Some(name) = key.strip_prefix('#') {
            if arguments.contains_key(name) {
                return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
                    .with_description(format!("both {name} and #{name} present")));
            }
            let reference: ResultReference =
                serde_json::from_value(value.clone()).map_err(|_| {
                    MethodError::new(error::method::INVALID_RESULT_REFERENCE)
                        .with_description(format!("#{name} is not a ResultReference"))
                })?;
            let source = responses
                .iter()
                .find(|response| {
                    response.call_id == reference.result_of && response.name == reference.name
                })
                .ok_or_else(|| {
                    MethodError::new(error::method::INVALID_RESULT_REFERENCE).with_description(
                        format!(
                            "no response {} for call id {}",
                            reference.name, reference.result_of
                        ),
                    )
                })?;
            let extracted = eval_pointer(&reference.path, &source.arguments).ok_or_else(|| {
                MethodError::new(error::method::INVALID_RESULT_REFERENCE)
                    .with_description(format!("path {} matched nothing", reference.path))
            })?;
            resolved.insert(name.to_owned(), extracted);
        } else {
            resolved.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(resolved))
}

/// Evaluate RFC 8620 §3.7 pointers.
/// Use JSON Pointer (RFC 6901) and `*` array mapping.
fn eval_pointer(path: &str, value: &Value) -> Option<Value> {
    fn walk(tokens: &[&str], value: &Value) -> Option<Value> {
        let Some((token, rest)) = tokens.split_first() else {
            return Some(value.clone());
        };
        if *token == "*" {
            let array = value.as_array()?;
            let mut collected = Vec::new();
            for item in array {
                match walk(rest, item)? {
                    Value::Array(nested) => collected.extend(nested),
                    single => collected.push(single),
                }
            }
            return Some(Value::Array(collected));
        }
        let unescaped = token.replace("~1", "/").replace("~0", "~");
        match value {
            Value::Object(map) => walk(rest, map.get(&unescaped)?),
            Value::Array(array) => walk(rest, array.get(unescaped.parse::<usize>().ok()?)?),
            _ => None,
        }
    }

    let trimmed = path.strip_prefix('/').unwrap_or(path);
    if trimmed.is_empty() {
        return Some(value.clone());
    }
    let tokens: Vec<&str> = trimmed.split('/').collect();
    walk(&tokens, value)
}

/// Build a `StateChange` from changed counters.
/// Return `None` when no counters changed.
fn state_change_since(
    before: &TypeStateSnapshot,
    after: &TypeStateSnapshot,
) -> Option<jmap_proto::push::StateChange> {
    let mut changed: std::collections::BTreeMap<jmap_proto::Id, jmap_proto::push::TypeState> =
        std::collections::BTreeMap::new();
    for (account_id, after_types) in after {
        let before_types = before.get(account_id);
        let mut types = jmap_proto::push::TypeState::new();
        for (&type_name, &counter) in after_types {
            let prior = before_types.and_then(|before_types| before_types.get(type_name));
            if prior != Some(&counter) {
                types.insert(
                    type_name.to_owned(),
                    jmap_proto::State::new(counter.to_string()),
                );
            }
        }
        if !types.is_empty() {
            changed.insert(account_id.clone(), types);
        }
    }
    (!changed.is_empty()).then(|| jmap_proto::push::StateChange::new(changed))
}

/// Collect `{creationId: {id: ...}}` pairs from a successful `/set` response.
fn record_created_ids(
    name: &str,
    result: &Value,
    created_ids: &mut std::collections::BTreeMap<String, jmap_proto::Id>,
) {
    if !name.ends_with("/set") {
        return;
    }
    let Some(created) = result.get("created").and_then(Value::as_object) else {
        return;
    };
    for (creation_id, object) in created {
        if let Some(id) = object.get("id").and_then(Value::as_str) {
            created_ids.insert(creation_id.clone(), jmap_proto::Id::new(id));
        }
    }
}

/// Dispatch one method invocation to its handler.
fn handle_method(
    state: &mut ServerState,
    name: &str,
    arguments: Value,
    created_ids: &std::collections::BTreeMap<String, jmap_proto::Id>,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    match name {
        "Core/echo" => Ok(arguments),
        "Mailbox/get" => crate::mail::mailbox_get(state, arguments, caller),
        "Mailbox/set" => crate::mail::mailbox_set(state, arguments, caller),
        "Email/get" => crate::mail::email_get(state, arguments),
        "Thread/get" => crate::mail::thread_get(state, arguments),
        "SearchSnippet/get" => crate::mail::search_snippet_get(state, arguments),
        "Email/query" => crate::mail::email_query(state, arguments),
        "Email/set" => crate::mail::email_set(state, arguments),
        "Email/import" => crate::mail::email_import(state, arguments),
        "Email/parse" => crate::mail::email_parse(state, arguments),
        "AddressBook/get" => crate::contacts::address_book_get(state, arguments, caller),
        "AddressBook/set" => crate::contacts::address_book_set(state, arguments, caller),
        "ContactCard/get" => crate::contacts::contact_card_get(state, arguments),
        "ContactCard/set" => crate::contacts::contact_card_set(state, arguments),
        "ContactCard/query" => crate::contacts::contact_card_query(state, arguments),
        "ContactCard/parse" => crate::contacts::contact_card_parse(state, arguments),
        "Mailbox/changes"
        | "Email/changes"
        | "Thread/changes"
        | "AddressBook/changes"
        | "ContactCard/changes"
        | "Calendar/changes"
        | "CalendarEvent/changes"
        | "EmailSubmission/changes"
        | "ParticipantIdentity/changes" => {
            let request: jmap_proto::methods::ChangesRequest = parse_arguments(arguments)?;
            let page_size = state.changes_page_size;
            let never_terminates = state.changes_never_terminates;
            let unrecognized_state_shape = state.stalwart_shaped_unrecognized_changes_state;
            let account = account_mut(state, &request.account_id)?;
            let mut response = match name {
                "Mailbox/changes" => crate::setops::store_changes(
                    &account.mailboxes,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                "Email/changes" => crate::setops::store_changes(
                    &account.emails,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                "Thread/changes" => crate::setops::store_changes(
                    &account.threads,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                "AddressBook/changes" => crate::setops::store_changes(
                    &account.address_books,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                "ContactCard/changes" => crate::setops::store_changes(
                    &account.contact_cards,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                "Calendar/changes" => crate::setops::store_changes(
                    &account.calendars,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                "EmailSubmission/changes" => crate::setops::store_changes(
                    &account.submissions,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                "ParticipantIdentity/changes" => crate::setops::store_changes(
                    &account.participant_identities,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
                _ => crate::setops::store_changes(
                    &account.calendar_events,
                    request,
                    page_size,
                    unrecognized_state_shape,
                ),
            }?;
            if never_terminates
                && (page_size.is_some() || response.has_more_changes)
                && (!response.created.is_empty()
                    || !response.updated.is_empty()
                    || !response.destroyed.is_empty())
            {
                response.has_more_changes = true;
                response.new_state = response.old_state.clone();
            }
            to_result(&response)
        }
        "Calendar/get" => crate::calendars::calendar_get(state, arguments, caller),
        "Calendar/set" => crate::calendars::calendar_set(state, arguments, caller),
        "CalendarEvent/get" => crate::calendars::calendar_event_get(state, arguments),
        "CalendarEvent/set" => crate::calendars::calendar_event_set(state, arguments, caller),
        "CalendarEvent/query" => crate::calendars::calendar_event_query(state, arguments),
        "CalendarEvent/parse" => crate::calendars::calendar_event_parse(state, arguments),
        "CalendarEventNotification/get" => {
            crate::calendars::calendar_event_notification_get(state, arguments, caller)
        }
        "CalendarEventNotification/query" => {
            crate::calendars::calendar_event_notification_query(state, arguments, caller)
        }
        "CalendarEventNotification/set" => {
            crate::calendars::calendar_event_notification_set(state, arguments, caller)
        }
        "ParticipantIdentity/get" => crate::calendars::participant_identity_get(state, arguments),
        "ParticipantIdentity/set" => crate::calendars::participant_identity_set(state, arguments),
        "Principal/get" => crate::principals::principal_get(state, arguments),
        "Principal/query" => crate::principals::principal_query(state, arguments),
        "Principal/getAvailability" => {
            crate::principals::principal_get_availability(state, arguments)
        }
        "ShareNotification/get" => {
            crate::principals::share_notification_get(state, arguments, caller)
        }
        "ShareNotification/query" => {
            crate::principals::share_notification_query(state, arguments, caller)
        }
        "Identity/get" => crate::mail::identity_get(state, arguments),
        "EmailSubmission/get" => crate::mail::email_submission_get(state, arguments),
        "EmailSubmission/set" => crate::mail::email_submission_set(state, arguments, created_ids),
        "EmailSubmission/query" => crate::mail::email_submission_query(state, arguments),
        "VacationResponse/get" => crate::mail::vacation_response_get(state, arguments),
        "VacationResponse/set" => crate::mail::vacation_response_set(state, arguments),
        "Quota/get" => crate::quota::quota_get(state, arguments),
        "Quota/query" => crate::quota::quota_query(state, arguments),
        "Blob/get" => crate::blob::blob_get(state, arguments),
        "Blob/upload" => crate::blob::blob_upload(state, arguments),
        "Blob/lookup" => crate::blob::blob_lookup(state, arguments),
        "Blob/copy" => crate::blob::blob_copy(state, arguments),
        "SieveScript/get" => crate::sieve::sieve_script_get(state, arguments),
        "SieveScript/set" => crate::sieve::sieve_script_set(state, arguments),
        "SieveScript/query" => crate::sieve::sieve_script_query(state, arguments),
        "SieveScript/validate" => crate::sieve::sieve_script_validate(state, arguments),
        _ => Err(MethodError::new(error::method::UNKNOWN_METHOD)),
    }
}

/// Parse method arguments into a typed request.
/// Return `invalidArguments` when parsing fails.
pub(crate) fn parse_arguments<T: serde::de::DeserializeOwned>(
    arguments: Value,
) -> Result<T, MethodError> {
    serde_json::from_value(arguments).map_err(|parse_error| {
        MethodError::new(error::method::INVALID_ARGUMENTS).with_description(parse_error.to_string())
    })
}

/// Serialize a typed response.
/// Treat serialization failure as a server bug.
pub(crate) fn to_result(response: &impl serde::Serialize) -> Result<Value, MethodError> {
    serde_json::to_value(response).map_err(|serialize_error| {
        MethodError::new(error::method::SERVER_FAIL).with_description(serialize_error.to_string())
    })
}

/// Look up an account, or return `accountNotFound`.
pub(crate) fn account_mut<'a>(
    state: &'a mut ServerState,
    account_id: &jmap_proto::Id,
) -> Result<&'a mut crate::state::AccountState, MethodError> {
    state
        .account_mut(account_id)
        .ok_or_else(|| MethodError::new(error::method::ACCOUNT_NOT_FOUND))
}

/// Apply `/get` projection and always keep `id`.
pub(crate) fn project_properties(
    object: &impl serde::Serialize,
    properties: Option<&[String]>,
) -> Result<Value, MethodError> {
    let full = to_result(object)?;
    let Some(properties) = properties else {
        return Ok(full);
    };
    let Value::Object(map) = full else {
        return Ok(full);
    };
    let filtered = map
        .into_iter()
        .filter(|(key, _)| key == "id" || properties.iter().any(|property| property == key))
        .collect();
    Ok(Value::Object(filtered))
}

#[cfg(test)]
mod tests {
    use super::{eval_pointer, resolve_references};
    use crate::state::ServerState;
    use jmap_proto::request::{Invocation, Request};
    use proptest::prelude::*;
    use serde_json::Value;
    use serde_json::json;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn json_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| Value::Number(n.into())),
            ".{0,16}".prop_map(Value::String),
        ];
        leaf.prop_recursive(4, 64, 8, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
                prop::collection::btree_map(".{0,8}", inner, 0..8)
                    .prop_map(|map| Value::Object(map.into_iter().collect())),
            ]
        })
    }

    fn method_name() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("Core/echo".to_owned()),
            Just("Mailbox/get".to_owned()),
            Just("Mailbox/set".to_owned()),
            Just("Email/get".to_owned()),
            Just("Email/query".to_owned()),
            Just("Email/set".to_owned()),
            Just("Email/import".to_owned()),
            Just("EmailSubmission/set".to_owned()),
            Just("AddressBook/get".to_owned()),
            Just("AddressBook/set".to_owned()),
            Just("ContactCard/get".to_owned()),
            Just("ContactCard/query".to_owned()),
            Just("Calendar/get".to_owned()),
            Just("Calendar/set".to_owned()),
            Just("CalendarEvent/get".to_owned()),
            Just("CalendarEvent/set".to_owned()),
            Just("Blob/upload".to_owned()),
            Just("Quota/get".to_owned()),
            ".{0,24}",
        ]
    }

    fn structured_request_body() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec((method_name(), json_value(), ".{0,8}"), 0..4).prop_map(|calls| {
            let method_calls: Vec<Value> = calls
                .into_iter()
                .map(|(name, arguments, call_id)| {
                    Value::Array(vec![Value::String(name), arguments, Value::String(call_id)])
                })
                .collect();
            serde_json::to_vec(&json!({
                "using": ["urn:ietf:params:jmap:core"],
                "methodCalls": method_calls,
            }))
            .expect("generated request body serializes")
        })
    }

    fn arbitrary_request_body() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            prop::collection::vec(any::<u8>(), 0..512),
            structured_request_body(),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn dispatcher_never_panics_on_bad_request_body(body in arbitrary_request_body()) {
            let mut state = ServerState::new();
            state.add_account("A1", "test");

            let result = catch_unwind(AssertUnwindSafe(|| {
                super::handle_api(&mut state, &body, None)
            }));

            prop_assert!(result.is_ok());
        }
    }

    #[test]
    fn pointer_object_and_array() {
        let value = json!({"ids": ["a", "b"], "list": [{"id": "x"}, {"id": "y"}]});
        assert_eq!(eval_pointer("/ids", &value), Some(json!(["a", "b"])));
        assert_eq!(eval_pointer("/ids/1", &value), Some(json!("b")));
        assert_eq!(eval_pointer("/list/*/id", &value), Some(json!(["x", "y"])));
        assert_eq!(eval_pointer("/missing", &value), None);
    }

    #[test]
    fn result_references_require_matching_call_id_and_method_name() {
        let call = Invocation::from_value(
            "Email/get",
            json!({
                "#ids": {
                    "resultOf": "c1",
                    "name": "Email/get",
                    "path": "/list"
                }
            }),
            "c2",
        );
        let responses = vec![
            Invocation::from_value("Mailbox/get", json!({"list": ["mailbox-id"]}), "c1"),
            Invocation::from_value("Email/get", json!({"list": ["wrong-call-id"]}), "c9"),
        ];

        let error = resolve_references(&call, &responses)
            .expect_err("partial response matches must not resolve a result reference");
        assert_eq!(
            error.error_type,
            jmap_proto::error::method::INVALID_RESULT_REFERENCE
        );
    }

    #[test]
    fn a_mailbox_set_create_pushes_a_state_change_automatically() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");
        let receiver = state.event_source.subscribe(None);

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call(
                "Mailbox/set",
                &json!({"accountId": "A1", "create": {"m1": {"name": "Inbox"}}}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, _response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);

        let pushed = receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("a StateChange was pushed automatically");
        let text = String::from_utf8(pushed).expect("utf8 SSE frame");
        assert!(text.starts_with("event: state\n"), "{text}");
        assert!(text.contains("\"A1\""), "{text}");
        assert!(text.contains("\"Mailbox\":\"2\""), "{text}");
    }

    #[test]
    fn a_request_that_mutates_nothing_pushes_nothing() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");
        let receiver = state.event_source.subscribe(None);

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, _response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);

        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn a_subscriber_filtered_to_an_unrelated_type_receives_nothing() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");
        let receiver = state
            .event_source
            .subscribe(Some(std::collections::BTreeSet::from([
                "ContactCard".to_owned()
            ])));

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call(
                "Mailbox/set",
                &json!({"accountId": "A1", "create": {"m1": {"name": "Inbox"}}}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, _response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);

        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn a_subscriber_filtered_to_the_changed_type_receives_it() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");
        let receiver = state
            .event_source
            .subscribe(Some(std::collections::BTreeSet::from([
                "Mailbox".to_owned()
            ])));

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call(
                "Mailbox/set",
                &json!({"accountId": "A1", "create": {"m1": {"name": "Inbox"}}}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, _response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);

        let pushed = receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("a StateChange was pushed to the matching subscriber");
        let text = String::from_utf8(pushed).expect("utf8 SSE frame");
        assert!(text.contains("\"Mailbox\":\"2\""), "{text}");
    }

    #[test]
    fn api_request_counter_increments_for_each_post() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let good_body = serde_json::to_vec(&request).expect("serialize request");

        let (status, _response) = super::handle_api(&mut state, &good_body, None);
        assert_eq!(status, 200);

        let (status, _response) = super::handle_api(&mut state, b"{not-json", None);
        assert_eq!(status, 400);

        assert_eq!(state.api_requests, 2);
    }

    #[test]
    fn requests_over_max_size_are_rejected() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");
        state.size_request = Some((body.len() - 1) as u64);

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 400);
        assert_eq!(
            response.get("type"),
            Some(&json!("urn:ietf:params:jmap:error:limit"))
        );
        assert_eq!(response.get("limit"), Some(&json!("maxSizeRequest")));
    }

    #[test]
    fn requests_at_max_size_are_accepted() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");
        state.size_request = Some(body.len() as u64);

        let (status, _response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
    }

    #[test]
    fn requests_over_max_calls_are_rejected() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request")
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c2")
            .expect("append request call");
        let body = serde_json::to_vec(&request).expect("serialize request");
        state.calls_in_request = Some(1);

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 400);
        assert_eq!(
            response.get("type"),
            Some(&json!("urn:ietf:params:jmap:error:limit"))
        );
        assert_eq!(response.get("limit"), Some(&json!("maxCallsInRequest")));
    }

    #[test]
    fn requests_at_max_calls_are_accepted() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");
        state.calls_in_request = Some(1);

        let (status, _response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
    }

    #[test]
    fn core_echo_returns_arguments_unchanged() {
        let mut state = ServerState::new();
        let arguments = json!({
            "message": "ping",
            "ids": ["A1", "A2"],
            "metadata": {"ok": true}
        });

        let request = Request::new(["urn:ietf:params:jmap:core"])
            .call("Core/echo", &arguments, "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("Core/echo"));
        assert_eq!(response["methodResponses"][0][1], arguments);
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn mailbox_get_dispatches_through_mailbox_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Mailbox/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("Mailbox/get"));
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn thread_get_dispatches_through_thread_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call("Thread/get", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("Thread/get"));
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn search_snippet_get_dispatches_through_search_snippet_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call(
                "SearchSnippet/get",
                &json!({"accountId": "A1", "emailIds": []}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(
            response["methodResponses"][0][0],
            json!("SearchSnippet/get")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn email_import_dispatches_through_email_import_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call(
                "Email/import",
                &json!({"accountId": "A1", "emails": {}}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("Email/import"));
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn email_parse_dispatches_through_email_parse_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call(
                "Email/parse",
                &json!({"accountId": "A1", "blobIds": []}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("Email/parse"));
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn address_book_set_dispatches_through_address_book_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"])
            .call(
                "AddressBook/set",
                &json!({"accountId": "A1", "create": {"book1": {"name": "Personal"}}}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("AddressBook/set"));
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn calendar_set_dispatches_through_calendar_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
        ])
        .call(
            "Calendar/set",
            &json!({"accountId": "A1", "create": {"cal1": {"name": "Work"}}}),
            "c1",
        )
        .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("Calendar/set"));
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn calendar_event_query_dispatches_through_calendar_event_query_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
        ])
        .call("CalendarEvent/query", &json!({"accountId": "A1"}), "c1")
        .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(
            response["methodResponses"][0][0],
            json!("CalendarEvent/query")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[cfg(feature = "parse")]
    #[test]
    fn calendar_event_parse_dispatches_through_calendar_event_parse_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
        ])
        .call(
            "CalendarEvent/parse",
            &json!({"accountId": "A1", "blobIds": []}),
            "c1",
        )
        .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(
            response["methodResponses"][0][0],
            json!("CalendarEvent/parse")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn calendar_event_notification_get_dispatches_through_notification_get_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
        ])
        .call(
            "CalendarEventNotification/get",
            &json!({"accountId": "A1"}),
            "c1",
        )
        .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(
            response["methodResponses"][0][0],
            json!("CalendarEventNotification/get")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn calendar_event_notification_query_dispatches_through_notification_query_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
        ])
        .call(
            "CalendarEventNotification/query",
            &json!({"accountId": "A1"}),
            "c1",
        )
        .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(
            response["methodResponses"][0][0],
            json!("CalendarEventNotification/query")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn contact_card_query_dispatches_through_contact_card_query_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"])
            .call("ContactCard/query", &json!({"accountId": "A1"}), "c1")
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(
            response["methodResponses"][0][0],
            json!("ContactCard/query")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[cfg(feature = "parse")]
    #[test]
    fn contact_card_parse_dispatches_through_contact_card_parse_handler() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"])
            .call(
                "ContactCard/parse",
                &json!({"accountId": "A1", "blobIds": []}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(
            response["methodResponses"][0][0],
            json!("ContactCard/parse")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[cfg(not(feature = "parse"))]
    #[test]
    fn calendar_event_parse_returns_unknown_method_when_parse_feature_is_disabled() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
        ])
        .call(
            "CalendarEvent/parse",
            &json!({"accountId": "A1", "blobIds": []}),
            "c1",
        )
        .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("error"));
        assert_eq!(
            response["methodResponses"][0][1]["type"],
            json!("unknownMethod")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[cfg(not(feature = "parse"))]
    #[test]
    fn contact_card_parse_returns_unknown_method_when_parse_feature_is_disabled() {
        let mut state = ServerState::new();
        state.add_account("A1", "test");

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"])
            .call(
                "ContactCard/parse",
                &json!({"accountId": "A1", "blobIds": []}),
                "c1",
            )
            .expect("build request");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);
        assert_eq!(response["methodResponses"][0][0], json!("error"));
        assert_eq!(
            response["methodResponses"][0][1]["type"],
            json!("unknownMethod")
        );
        assert_eq!(response["methodResponses"][0][2], json!("c1"));
    }

    #[test]
    fn email_submission_set_resolves_email_creation_id_from_prior_set_call() {
        let mut state = ServerState::new();
        let account = state.add_account("A1", "test");
        let identity_id = account.seed_identity("Sender", "sender@example.test");
        let inbox_id = account.seed_mailbox("Inbox", Some("inbox"));

        let request = Request::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
            .call(
                "Email/set",
                &json!({
                    "accountId": "A1",
                    "create": {
                        "draft": {
                            "mailboxIds": {
                                (inbox_id.to_string()): true
                            }
                        }
                    }
                }),
                "c1",
            )
            .expect("build request")
            .call(
                "EmailSubmission/set",
                &json!({
                    "accountId": "A1",
                    "create": {
                        "submit1": {
                            "identityId": identity_id,
                            "emailId": "#draft"
                        }
                    }
                }),
                "c2",
            )
            .expect("append request call");
        let body = serde_json::to_vec(&request).expect("serialize request");

        let (status, response) = super::handle_api(&mut state, &body, None);
        assert_eq!(status, 200);

        let created_email_id = response["methodResponses"][0][1]["created"]["draft"]["id"]
            .as_str()
            .expect("Email/set create returns an id")
            .to_owned();
        let submission = &response["methodResponses"][1];
        assert_eq!(submission[0], json!("EmailSubmission/set"));
        assert!(submission[1].get("notCreated").is_none());
        assert_eq!(
            submission[1]["created"]["submit1"]["emailId"],
            json!(created_email_id)
        );
    }
}
