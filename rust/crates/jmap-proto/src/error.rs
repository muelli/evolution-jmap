// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Error objects: request-level (RFC 8620 §3.6.1, RFC 7807 problem details),
//! method-level (§3.6.2), and set-level (§5.3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request-level error, returned with an HTTP 4xx and
/// `application/problem+json`. Extension members (e.g. `limit`) are kept in
/// `extra` so nothing is lost on a round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestError {
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Arguments of a method-level `error` response (RFC 8620 §3.6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodError {
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MethodError {
    pub fn new(error_type: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            description: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// A per-record failure inside a `/set` response (RFC 8620 §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetError {
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SetError {
    pub fn new(error_type: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            description: None,
            properties: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_properties<P: Into<String>>(
        mut self,
        properties: impl IntoIterator<Item = P>,
    ) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }
}

/// Well-known method-level error types (RFC 8620 §3.6.2).
pub mod method {
    pub const UNKNOWN_METHOD: &str = "unknownMethod";
    pub const INVALID_ARGUMENTS: &str = "invalidArguments";
    pub const INVALID_RESULT_REFERENCE: &str = "invalidResultReference";
    pub const FORBIDDEN: &str = "forbidden";
    pub const ACCOUNT_NOT_FOUND: &str = "accountNotFound";
    pub const ACCOUNT_NOT_SUPPORTED_BY_METHOD: &str = "accountNotSupportedByMethod";
    pub const ACCOUNT_READ_ONLY: &str = "accountReadOnly";
    pub const SERVER_FAIL: &str = "serverFail";
    pub const SERVER_UNAVAILABLE: &str = "serverUnavailable";
    pub const SERVER_PARTIAL_FAIL: &str = "serverPartialFail";
    pub const UNKNOWN_CAPABILITY: &str = "unknownCapability";
    pub const STATE_MISMATCH: &str = "stateMismatch";
    /// The `since_state` of a `/changes` call is too old for the server to
    /// answer from its log; the client must resynchronise in full.
    pub const CANNOT_CALCULATE_CHANGES: &str = "cannotCalculateChanges";
    /// The call asked for more objects than the server is willing to answer in
    /// one — for `/get`, more ids than `maxObjectsInGet` (RFC 8620 §5.1). Not a
    /// condition to report to the user: a client that reads the session
    /// document's limits and keeps to them never sees it.
    pub const REQUEST_TOO_LARGE: &str = "requestTooLarge";
}

/// Well-known set-level error types (RFC 8620 §5.3).
pub mod set {
    pub const FORBIDDEN: &str = "forbidden";
    pub const OVER_QUOTA: &str = "overQuota";
    pub const TOO_LARGE: &str = "tooLarge";
    pub const RATE_LIMIT: &str = "rateLimit";
    pub const NOT_FOUND: &str = "notFound";
    pub const INVALID_PATCH: &str = "invalidPatch";
    pub const INVALID_PROPERTIES: &str = "invalidProperties";
    pub const SINGLETON: &str = "singleton";
    pub const WILL_DESTROY: &str = "willDestroy";
    pub const STATE_MISMATCH: &str = "stateMismatch";
    pub const REQUEST_TOO_LARGE: &str = "requestTooLarge";
}

/// Standard RFC 8620 §3.6.1 request-level problem types (RFC 7807 URI format).
pub mod request {
    pub const UNKNOWN_CAPABILITY: &str = "urn:ietf:params:jmap:error:unknownCapability";
    pub const NOT_JSON: &str = "urn:ietf:params:jmap:error:notJSON";
    pub const NOT_REQUEST: &str = "urn:ietf:params:jmap:error:notRequest";
    pub const LIMIT: &str = "urn:ietf:params:jmap:error:limit";
}
