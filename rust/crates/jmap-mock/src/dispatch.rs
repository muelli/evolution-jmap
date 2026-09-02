// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! API endpoint dispatch: envelope parsing, back-reference resolution
//! (RFC 8620 §3.7), and the method registry.

use jmap_proto::Id;
use jmap_proto::error::{self, MethodError};
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use serde_json::{Map, Value};

use crate::state::{ServerState, TypeStateSnapshot};

/// Handle a POST to the API endpoint. Returns (HTTP status, JSON body).
///
/// `caller` is the principal the request's credential was bound to via
/// [`crate::auth::AuthConfig::allow_bearer_as`], for the cross-account
/// sharing checks (Track E Phase C) that need to know who is asking; `None`
/// means the credential named no identity, which every check before sharing
/// existed already assumed and keeps assuming.
pub fn handle_api(state: &mut ServerState, body: &[u8], caller: Option<&Id>) -> (u16, Value) {
    if let Ok(text) = std::str::from_utf8(body) {
        println!("--> POST /jmap\n{text}");
    }

    // Counted before anything is read out of the body: what it is for is
    // telling one request carrying two chained calls apart from two requests
    // carrying one each, and a request refused below is still a round trip the
    // client spent.
    state.api_requests += 1;

    // RFC 8620 §2: over `maxSizeRequest` the request is refused on its octets,
    // before it is a request at all — a server counting bytes has not parsed
    // anything yet, and cannot have run any of the calls inside.
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

    // RFC 8620 §3.2: over `maxCallsInRequest` the whole request is refused with
    // a request-level error, and nothing in it runs — not even the calls that
    // would have fitted.
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
    // Creation-id → real id across the whole request, so later /set calls
    // can reference earlier creations as `#creationId` (RFC 8620 §5.3).
    let mut created_ids: std::collections::BTreeMap<String, jmap_proto::Id> =
        std::collections::BTreeMap::new();
    // Snapshotted before any call runs, and diffed against the same
    // snapshot taken after (below), so this request can push one
    // `StateChange` (RFC 8620 §7.1) for whatever it actually mutated —
    // wiring a real mutation to `push_state_change` automatically, without
    // every `*/set` handler having to say so itself.
    let type_states_before = state.type_state_snapshot();
    for call in &request.method_calls {
        // Recorded before the call is answered, and whatever the answer is: a
        // request that failed is still a round trip the client spent, which is
        // what a test counting them is asking about.
        state.method_calls.push(call.name.clone());
        let invocation = match resolve_references(call, &responses) {
            Ok(arguments) => {
                match handle_method(state, &call.name, arguments, &created_ids, caller) {
                    Ok(result) => {
                        record_created_ids(&call.name, &result, &mut created_ids);
                        Invocation {
                            name: call.name.clone(),
                            arguments: result,
                            call_id: call.call_id.clone(),
                        }
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

    let response = Response {
        method_responses: responses,
        created_ids: None,
        session_state: state.session_state(),
    };

    let response_value = serde_json::to_value(&response).expect("response serializes");
    println!(
        "<-- 200 OK\n{}\n",
        serde_json::to_string_pretty(&response_value).unwrap_or_default()
    );

    (200, response_value)
}

fn error_invocation(error: MethodError, call_id: &str) -> Invocation {
    Invocation {
        name: "error".to_owned(),
        arguments: serde_json::to_value(&error).expect("error serializes"),
        call_id: call_id.to_owned(),
    }
}

/// Replace `#argument` result references with values extracted from earlier
/// responses in the same request (RFC 8620 §3.7).
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

/// RFC 8620 §3.7 pointer evaluation: JSON pointer (RFC 6901) extended with a
/// `*` token that maps over arrays and flattens one level of nested arrays.
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

/// Diff two [`ServerState::type_state_snapshot`] snapshots into a
/// `StateChange` naming only the accounts/types whose counter actually
/// moved between them — `None` when nothing did, so a request that mutated
/// nothing (a `/get`, a failed `/set`) pushes nothing.
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

/// Collect `{creationId: {id: ...}}` pairs from a successful `/set` result.
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

/// The method registry. Domain modules add their arms as milestones land.
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
        "Mailbox/set" => crate::mail::mailbox_set(state, arguments),
        "Email/get" => crate::mail::email_get(state, arguments),
        "Thread/get" => crate::mail::thread_get(state, arguments),
        "SearchSnippet/get" => crate::mail::search_snippet_get(state, arguments),
        "Email/query" => crate::mail::email_query(state, arguments),
        "Email/set" => crate::mail::email_set(state, arguments),
        "Email/import" => crate::mail::email_import(state, arguments),
        "AddressBook/get" => crate::contacts::address_book_get(state, arguments, caller),
        "AddressBook/set" => crate::contacts::address_book_set(state, arguments),
        "ContactCard/get" => crate::contacts::contact_card_get(state, arguments),
        "ContactCard/set" => crate::contacts::contact_card_set(state, arguments),
        "ContactCard/query" => crate::contacts::contact_card_query(state, arguments),
        "Mailbox/changes"
        | "Email/changes"
        | "AddressBook/changes"
        | "ContactCard/changes"
        | "Calendar/changes"
        | "CalendarEvent/changes" => {
            let request: jmap_proto::methods::ChangesRequest = parse_arguments(arguments)?;
            let page_size = state.changes_page_size;
            let account = account_mut(state, &request.account_id)?;
            let response = match name {
                "Mailbox/changes" => {
                    crate::setops::store_changes(&account.mailboxes, request, page_size)
                }
                "Email/changes" => {
                    crate::setops::store_changes(&account.emails, request, page_size)
                }
                "AddressBook/changes" => {
                    crate::setops::store_changes(&account.address_books, request, page_size)
                }
                "ContactCard/changes" => {
                    crate::setops::store_changes(&account.contact_cards, request, page_size)
                }
                "Calendar/changes" => {
                    crate::setops::store_changes(&account.calendars, request, page_size)
                }
                _ => crate::setops::store_changes(&account.calendar_events, request, page_size),
            }?;
            to_result(&response)
        }
        "Calendar/get" => crate::calendars::calendar_get(state, arguments, caller),
        "Calendar/set" => crate::calendars::calendar_set(state, arguments),
        "CalendarEvent/get" => crate::calendars::calendar_event_get(state, arguments),
        "CalendarEvent/set" => crate::calendars::calendar_event_set(state, arguments),
        "CalendarEvent/query" => crate::calendars::calendar_event_query(state, arguments),
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
        "EmailSubmission/set" => crate::mail::email_submission_set(state, arguments, created_ids),
        "VacationResponse/get" => crate::mail::vacation_response_get(state, arguments),
        "VacationResponse/set" => crate::mail::vacation_response_set(state, arguments),
        "Quota/get" => crate::quota::quota_get(state, arguments),
        "SieveScript/get" => crate::sieve::sieve_script_get(state, arguments),
        _ => Err(MethodError::new(error::method::UNKNOWN_METHOD)),
    }
}

// ── Helpers shared by method handlers ────────────────────────────────────────

/// Parse method arguments into a typed request, mapping failures to
/// `invalidArguments`.
pub(crate) fn parse_arguments<T: serde::de::DeserializeOwned>(
    arguments: Value,
) -> Result<T, MethodError> {
    serde_json::from_value(arguments).map_err(|parse_error| {
        MethodError::new(error::method::INVALID_ARGUMENTS).with_description(parse_error.to_string())
    })
}

/// Serialize a typed response, treating failure as a server bug.
pub(crate) fn to_result(response: &impl serde::Serialize) -> Result<Value, MethodError> {
    serde_json::to_value(response).map_err(|serialize_error| {
        MethodError::new(error::method::SERVER_FAIL).with_description(serialize_error.to_string())
    })
}

/// Look up an account or fail with `accountNotFound`.
pub(crate) fn account_mut<'a>(
    state: &'a mut ServerState,
    account_id: &jmap_proto::Id,
) -> Result<&'a mut crate::state::AccountState, MethodError> {
    state
        .account_mut(account_id)
        .ok_or_else(|| MethodError::new(error::method::ACCOUNT_NOT_FOUND))
}

/// Apply `/get` property projection: keep only the requested properties
/// (plus `id`, which is always returned).
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
    use super::eval_pointer;
    use crate::state::ServerState;
    use jmap_proto::request::Request;
    use serde_json::json;

    #[test]
    fn pointer_object_and_array() {
        let value = json!({"ids": ["a", "b"], "list": [{"id": "x"}, {"id": "y"}]});
        assert_eq!(eval_pointer("/ids", &value), Some(json!(["a", "b"])));
        assert_eq!(eval_pointer("/ids/1", &value), Some(json!("b")));
        assert_eq!(eval_pointer("/list/*/id", &value), Some(json!(["x", "y"])));
        assert_eq!(eval_pointer("/missing", &value), None);
    }

    /// A real mutation should push a `StateChange` on its own, not only
    /// through the `MockServer::push_state_change` test hook. A subscriber
    /// registered directly against the state's own hub (no socket needed —
    /// that plumbing is `eventsource`'s own concern, already tested there)
    /// proves `handle_api` itself is the one broadcasting.
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

    /// The other half of the same guarantee: a request that mutates nothing
    /// (here, a `Mailbox/get` on an account with none) must push nothing —
    /// a subscriber has no way to distinguish "no change" from "a slow
    /// server", so a false push would train a client to distrust real ones.
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

    /// Push's `types`-filtering: a subscriber whose `types` filter names
    /// none of what actually changed is sent nothing at all, same as a
    /// request that mutated nothing.
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

    /// The other half: a subscriber filtered to a type that DID change still
    /// receives the push.
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
}
