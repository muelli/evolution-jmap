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
    pub const REQUEST: &str = "Request";
    pub const RESPONSE: &str = "Response";
    pub const REQUEST_ERROR: &str = "RequestError";
    pub const PUSH_ENABLE: &str = "WebSocketPushEnable";
    pub const PUSH_DISABLE: &str = "WebSocketPushDisable";
    pub const STATE_CHANGE: &str = "StateChange";
}

/// A client-to-server request frame over WebSocket (RFC 8887 §2.2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketRequest {
    #[serde(rename = "@type")]
    pub message_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub using: Vec<String>,
    pub method_calls: Vec<Invocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WebSocketRequest {
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

    pub fn with_invocation(mut self, invocation: Invocation) -> Self {
        self.method_calls.push(invocation);
        self
    }

    pub fn with_created_ids(mut self, created_ids: BTreeMap<Id, Id>) -> Self {
        self.created_ids = Some(created_ids);
        self
    }
}

/// A server-to-client response frame over WebSocket (RFC 8887 §2.2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketResponse {
    #[serde(rename = "@type")]
    pub message_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default)]
    pub method_responses: Vec<Invocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_state: Option<State>,
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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_method_response(mut self, invocation: Invocation) -> Self {
        self.method_responses.push(invocation);
        self
    }

    pub fn with_created_ids(mut self, created_ids: BTreeMap<Id, Id>) -> Self {
        self.created_ids = Some(created_ids);
        self
    }

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
pub struct WebSocketRequestError {
    #[serde(rename = "@type")]
    pub message_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WebSocketRequestError {
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

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A client request to enable WebSocket push notifications (RFC 8887 §2.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketPushEnable {
    #[serde(rename = "@type")]
    pub message_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_types: Option<Vec<String>>,
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
    pub fn new() -> Self {
        Self::default()
    }

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
pub struct WebSocketPushDisable {
    #[serde(rename = "@type")]
    pub message_type: String,
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
    pub fn new() -> Self {
        Self::default()
    }
}
