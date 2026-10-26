// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 8887 JMAP Subprotocol for WebSocket.
//!
//! Models text frames exchanged over the `jmap` WebSocket subprotocol:
//! `Request`, `Response`, `RequestError`, `WebSocketPushEnable`,
//! and `WebSocketPushDisable`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;
use crate::request::Invocation;
use crate::state::State;

/// The standard WebSocket subprotocol name (RFC 8887 §2.1).
pub const SUBPROTOCOL: &str = "jmap";

/// Standard RFC 8887 `@type` message type values.
pub mod message_type {
    /// The `@type` value for a client-to-server request frame.
    pub const REQUEST: &str = "Request";
    /// The `@type` value for a server-to-client response frame.
    pub const RESPONSE: &str = "Response";
    /// The `@type` value for a request-level error frame.
    pub const REQUEST_ERROR: &str = "RequestError";
    /// The `@type` value for a request to enable push notifications.
    pub const PUSH_ENABLE: &str = "WebSocketPushEnable";
    /// The `@type` value for a request to disable push notifications.
    pub const PUSH_DISABLE: &str = "WebSocketPushDisable";
    /// The `@type` value for a pushed state change notification.
    pub const STATE_CHANGE: &str = "StateChange";
}

/// A client-to-server request frame over WebSocket (RFC 8887 §2.2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebSocketRequest {
    /// The literal `@type` value, always `"Request"`.
    #[serde(rename = "@type")]
    pub message_type: String,
    /// A client-chosen id echoed back on the matching response frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The capability URIs this request uses.
    pub using: Vec<String>,
    /// The method calls to invoke, in order.
    pub method_calls: Vec<Invocation>,
    /// Client-chosen ids for objects this request may create, mapped to their place in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WebSocketRequest {
    /// Create a request using the given capability URIs, with no method calls yet.
    pub fn new(using: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            message_type: message_type::REQUEST.to_owned(),
            id: None,
            using: using.into_iter().map(Into::into).collect(),
            method_calls: Vec::new(),
            created_ids: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the client-chosen id echoed back on the matching response frame.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Append a method call with typed arguments.
    pub fn call(
        mut self,
        name: impl Into<String>,
        arguments: &impl Serialize,
        call_id: impl Into<String>,
    ) -> Result<Self, serde_json::Error> {
        self.method_calls
            .push(Invocation::new(name, arguments, call_id)?);
        Ok(self)
    }

    /// Append an already-built method call invocation.
    pub fn with_invocation(mut self, invocation: Invocation) -> Self {
        self.method_calls.push(invocation);
        self
    }

    /// Set the client-chosen ids for objects this request may create.
    pub fn with_created_ids(mut self, created_ids: BTreeMap<Id, Id>) -> Self {
        self.created_ids = Some(created_ids);
        self
    }
}

/// A server-to-client response frame over WebSocket (RFC 8887 §2.2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebSocketResponse {
    /// The literal `@type` value, always `"Response"`.
    #[serde(rename = "@type")]
    pub message_type: String,
    /// The id echoed back from the matching request frame, if it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The method responses, in the order the server executed them.
    #[serde(default)]
    pub method_responses: Vec<Invocation>,
    /// The client-chosen ids from the request, mapped to the server ids created for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
    /// The current session state, if the server included it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_state: Option<State>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for WebSocketResponse {
    fn default() -> Self {
        Self {
            message_type: message_type::RESPONSE.to_owned(),
            id: None,
            method_responses: Vec::new(),
            created_ids: None,
            session_state: None,
            extra: BTreeMap::new(),
        }
    }
}

impl WebSocketResponse {
    /// Create an empty response with no method responses yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the id echoed back from the matching request frame.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Append one method response.
    pub fn with_method_response(mut self, invocation: Invocation) -> Self {
        self.method_responses.push(invocation);
        self
    }

    /// Set the client-chosen ids mapped to the server ids created for them.
    pub fn with_created_ids(mut self, created_ids: BTreeMap<Id, Id>) -> Self {
        self.created_ids = Some(created_ids);
        self
    }

    /// Set the current session state.
    pub fn with_session_state(mut self, session_state: impl Into<State>) -> Self {
        self.session_state = Some(session_state.into());
        self
    }

    /// All responses belonging to the method call with the given call id.
    pub fn responses_for<'a>(&'a self, call_id: &'a str) -> impl Iterator<Item = &'a Invocation> {
        self.method_responses
            .iter()
            .filter(move |invocation| invocation.call_id == call_id)
    }
}

/// A request-level error frame over WebSocket (RFC 8887 §2.2.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WebSocketRequestError {
    /// The literal `@type` value, always `"RequestError"`.
    #[serde(rename = "@type")]
    pub message_type: String,
    /// The id echoed back from the matching request frame, if it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The problem type URI identifying the kind of error.
    #[serde(rename = "type")]
    pub error_type: String,
    /// The HTTP status code the error corresponds to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// A human-readable description of the error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WebSocketRequestError {
    /// Create an error of the given problem type.
    pub fn new(error_type: impl Into<String>) -> Self {
        Self {
            message_type: message_type::REQUEST_ERROR.to_owned(),
            id: None,
            error_type: error_type.into(),
            status: None,
            detail: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the id echoed back from the matching request frame.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the HTTP status code the error corresponds to.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Set a human-readable description of the error.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A client request to enable WebSocket push notifications (RFC 8887 §2.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebSocketPushEnable {
    /// The literal `@type` value, always `"WebSocketPushEnable"`.
    #[serde(rename = "@type")]
    pub message_type: String,
    /// The data types to push notifications for; `None` means all types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_types: Option<Vec<String>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for WebSocketPushEnable {
    fn default() -> Self {
        Self {
            message_type: message_type::PUSH_ENABLE.to_owned(),
            data_types: None,
            extra: BTreeMap::new(),
        }
    }
}

impl WebSocketPushEnable {
    /// Create a request enabling push for all data types.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict push notifications to the given data types.
    pub fn with_data_types(
        mut self,
        data_types: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.data_types = Some(data_types.into_iter().map(Into::into).collect());
        self
    }
}

/// A client request to disable WebSocket push notifications (RFC 8887 §2.3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebSocketPushDisable {
    /// The literal `@type` value, always `"WebSocketPushDisable"`.
    #[serde(rename = "@type")]
    pub message_type: String,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for WebSocketPushDisable {
    fn default() -> Self {
        Self {
            message_type: message_type::PUSH_DISABLE.to_owned(),
            extra: BTreeMap::new(),
        }
    }
}

impl WebSocketPushDisable {
    /// Create a request disabling push notifications.
    pub fn new() -> Self {
        Self::default()
    }
}
