// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! API endpoint dispatch: envelope parsing, back-reference resolution
//! (RFC 8620 §3.7), and the method registry.

use jmap_proto::error::{self, MethodError};
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use serde_json::{Map, Value};

use crate::state::ServerState;

/// Handle a POST to the API endpoint. Returns (HTTP status, JSON body).
pub fn handle_api(state: &mut ServerState, body: &[u8]) -> (u16, Value) {
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

    let mut responses: Vec<Invocation> = Vec::new();
    for call in &request.method_calls {
        let invocation = match resolve_references(call, &responses) {
            Ok(arguments) => match handle_method(state, &call.name, arguments) {
                Ok(result) => Invocation {
                    name: call.name.clone(),
                    arguments: result,
                    call_id: call.call_id.clone(),
                },
                Err(method_error) => error_invocation(method_error, &call.call_id),
            },
            Err(method_error) => error_invocation(method_error, &call.call_id),
        };
        responses.push(invocation);
    }

    let response = Response {
        method_responses: responses,
        created_ids: None,
        session_state: state.session_state(),
    };
    (
        200,
        serde_json::to_value(&response).expect("response serializes"),
    )
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

/// The method registry. Domain modules add their arms as milestones land.
fn handle_method(
    state: &mut ServerState,
    name: &str,
    arguments: Value,
) -> Result<Value, MethodError> {
    let _ = state;
    match name {
        "Core/echo" => Ok(arguments),
        _ => Err(MethodError::new(error::method::UNKNOWN_METHOD)),
    }
}

#[cfg(test)]
mod tests {
    use super::eval_pointer;
    use serde_json::json;

    #[test]
    fn pointer_object_and_array() {
        let value = json!({"ids": ["a", "b"], "list": [{"id": "x"}, {"id": "y"}]});
        assert_eq!(eval_pointer("/ids", &value), Some(json!(["a", "b"])));
        assert_eq!(eval_pointer("/ids/1", &value), Some(json!("b")));
        assert_eq!(eval_pointer("/list/*/id", &value), Some(json!(["x", "y"])));
        assert_eq!(eval_pointer("/missing", &value), None);
    }
}
