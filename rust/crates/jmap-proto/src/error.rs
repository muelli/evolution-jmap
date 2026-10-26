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
#[non_exhaustive]
pub struct RequestError {
    /// The problem type URI (RFC 7807 §3.1), usually one of this module's `request` constants.
    #[serde(rename = "type")]
    pub error_type: String,
    /// The HTTP status code repeated from the response header, if the server sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// A human-readable explanation of this specific occurrence, if the server sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl RequestError {
    /// Create a request-level error with only its problem type set.
    pub fn new(error_type: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            status: None,
            detail: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the HTTP status code to repeat in the body.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Set a human-readable explanation of this occurrence.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Arguments of a method-level `error` response (RFC 8620 §3.6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MethodError {
    /// The method-level error type, usually one of this module's `method` constants.
    #[serde(rename = "type")]
    pub error_type: String,
    /// A human-readable explanation of the error, if the server sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MethodError {
    /// Create a method-level error with only its error type set.
    pub fn new(error_type: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            description: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set a human-readable explanation of the error.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// A per-record failure inside a `/set` response (RFC 8620 §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SetError {
    /// The set-level error type, usually one of this module's `set` constants.
    #[serde(rename = "type")]
    pub error_type: String,
    /// A human-readable explanation of the error, if the server sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The specific properties that caused an `invalidProperties` error, if the server sent them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SetError {
    /// Create a set-level error with only its error type set.
    pub fn new(error_type: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            description: None,
            properties: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set a human-readable explanation of the error.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the properties that caused the error.
    pub fn with_properties<P: Into<String>>(
        mut self,
        properties: impl IntoIterator<Item = P>,
    ) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Well-known method-level error types (RFC 8620 §3.6.2).
pub mod method {
    /// The client called a method the server does not implement.
    pub const UNKNOWN_METHOD: &str = "unknownMethod";
    /// One of the method's arguments is invalid.
    pub const INVALID_ARGUMENTS: &str = "invalidArguments";
    /// A result reference in the call could not be resolved.
    pub const INVALID_RESULT_REFERENCE: &str = "invalidResultReference";
    /// The user has insufficient permission for this method call.
    pub const FORBIDDEN: &str = "forbidden";
    /// The `accountId` does not correspond to a valid account.
    pub const ACCOUNT_NOT_FOUND: &str = "accountNotFound";
    /// The account exists but does not support this data type.
    pub const ACCOUNT_NOT_SUPPORTED_BY_METHOD: &str = "accountNotSupportedByMethod";
    /// The method call would modify an account that is read-only.
    pub const ACCOUNT_READ_ONLY: &str = "accountReadOnly";
    /// An unexpected error occurred while processing the call.
    pub const SERVER_FAIL: &str = "serverFail";
    /// A sub-method is unavailable, for example due to a backend not responding.
    pub const SERVER_UNAVAILABLE: &str = "serverUnavailable";
    /// The server partially fulfilled the request; some changes were not made.
    pub const SERVER_PARTIAL_FAIL: &str = "serverPartialFail";
    /// A capability referenced in the call is not supported by this account.
    pub const UNKNOWN_CAPABILITY: &str = "unknownCapability";
    /// The `ifInState` argument did not match the current state of the data type.
    pub const STATE_MISMATCH: &str = "stateMismatch";
    /// The `fromAccountState` argument given to a copy-style call is out of date.
    pub const FROM_STATE_MISMATCH: &str = "fromStateMismatch";
    /// The `since_state` of a `/changes` call is too old for the server to
    /// answer from its log; the client must resynchronise in full.
    pub const CANNOT_CALCULATE_CHANGES: &str = "cannotCalculateChanges";
    /// The call asked for more objects than the server allows in one request (RFC 8620 §5.1).
    pub const REQUEST_TOO_LARGE: &str = "requestTooLarge";
    /// The `fromAccountId` of a copy-style call does not correspond to a valid account.
    pub const FROM_ACCOUNT_NOT_FOUND: &str = "fromAccountNotFound";
    /// The account named by `fromAccountId` does not support this data type.
    pub const FROM_ACCOUNT_NOT_SUPPORTED_BY_METHOD: &str = "fromAccountNotSupportedByMethod";
    /// The `/query` filter uses a condition the server does not support.
    pub const UNSUPPORTED_FILTER: &str = "unsupportedFilter";
    /// The `/query` sort uses a comparator the server does not support.
    pub const UNSUPPORTED_SORT: &str = "unsupportedSort";
    /// The `anchor` id given to a `/query` call does not match any record in the results.
    pub const ANCHOR_NOT_FOUND: &str = "anchorNotFound";
    /// Too many changes have happened since the given state for the server to summarise.
    pub const TOO_MANY_CHANGES: &str = "tooManyChanges";
    /// The data type named in the call is not known to the server.
    pub const UNKNOWN_DATA_TYPE: &str = "unknownDataType";
}

/// Well-known set-level error types (RFC 8620 §5.3, §5.4).
pub mod set {
    /// The user does not have permission to create, update or destroy this record.
    pub const FORBIDDEN: &str = "forbidden";
    /// The create or update would exceed a quota the server enforces.
    pub const OVER_QUOTA: &str = "overQuota";
    /// The record is larger than the server allows.
    pub const TOO_LARGE: &str = "tooLarge";
    /// Too many records of this type have been created recently.
    pub const RATE_LIMIT: &str = "rateLimit";
    /// The id given for update or destroy does not correspond to a record.
    pub const NOT_FOUND: &str = "notFound";
    /// The `/update` patch is invalid, for example it targets a read-only property.
    pub const INVALID_PATCH: &str = "invalidPatch";
    /// One or more properties in the record are invalid; see the error's `properties`.
    pub const INVALID_PROPERTIES: &str = "invalidProperties";
    /// The data type allows only one record and it already exists.
    pub const SINGLETON: &str = "singleton";
    /// Destroying this record would also destroy records that reference it.
    pub const WILL_DESTROY: &str = "willDestroy";
    /// The record's state changed since the `ifInState` argument was captured.
    pub const STATE_MISMATCH: &str = "stateMismatch";
    /// The `/set` call asked for more changes than the server allows in one request.
    pub const REQUEST_TOO_LARGE: &str = "requestTooLarge";
    /// A record with the same unique identifying property already exists.
    pub const ALREADY_EXISTS: &str = "alreadyExists";
    /// The record is the original of a copy and cannot be destroyed while the copy exists.
    pub const CANNOT_DESTROY_ORIGINAL: &str = "cannotDestroyOriginal";
    /// The record is the account's default and cannot be destroyed.
    pub const CANNOT_DESTROY_DEFAULT: &str = "cannotDestroyDefault";
}

/// Standard RFC 8620 §3.6.1 request-level problem types (RFC 7807 URI format).
pub mod request {
    /// The request used a capability URI the account does not support.
    pub const UNKNOWN_CAPABILITY: &str = "urn:ietf:params:jmap:error:unknownCapability";
    /// The request body is not valid JSON.
    pub const NOT_JSON: &str = "urn:ietf:params:jmap:error:notJSON";
    /// The request body is valid JSON but not a valid JMAP request object.
    pub const NOT_REQUEST: &str = "urn:ietf:params:jmap:error:notRequest";
    /// The request exceeds a server limit named in the error's `extra` properties.
    pub const LIMIT: &str = "urn:ietf:params:jmap:error:limit";
}
