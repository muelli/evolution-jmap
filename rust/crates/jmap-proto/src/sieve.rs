// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9661: JMAP for Sieve Scripts.
//!
//! Models the `SieveScript` data type, query filters, `SieveScript/validate`
//! method envelopes, capability object, and standard error codes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SetError;
use crate::id::Id;
use crate::methods::SetRequest;

/// The Sieve capability URI (RFC 9661 §1.2.1).
pub const CAPABILITY_SIEVE: &str = "urn:ietf:params:jmap:sieve";

/// Sieve capability properties (RFC 9661 §1.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SieveCapability {
    /// The largest Sieve script the server accepts, in octets.
    pub max_size_script: u64,
    /// The largest number of scripts the account may have, if the server limits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_number_scripts: Option<u64>,
    /// A free-text description of the Sieve engine running on the server, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<String>,
    /// The Sieve extensions the server supports, by their registered name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sieve_extensions: Vec<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveCapability {
    /// Create a capability with only the maximum script size set.
    pub fn new(max_size_script: u64) -> Self {
        Self {
            max_size_script,
            max_number_scripts: None,
            implementation: None,
            sieve_extensions: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the largest number of scripts the account may have.
    pub fn with_max_number_scripts(mut self, max: u64) -> Self {
        self.max_number_scripts = Some(max);
        self
    }

    /// Set a free-text description of the Sieve engine running on the server.
    pub fn with_implementation(mut self, implementation: impl Into<String>) -> Self {
        self.implementation = Some(implementation.into());
        self
    }

    /// Set the Sieve extensions the server supports, replacing any already set.
    pub fn with_sieve_extensions(
        mut self,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.sieve_extensions = extensions.into_iter().map(Into::into).collect();
        self
    }

    /// Add one Sieve extension the server supports.
    pub fn with_sieve_extension(mut self, extension: impl Into<String>) -> Self {
        self.sieve_extensions.push(extension.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

fn deserialize_null_as_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// A Sieve script object (RFC 9661 §2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SieveScript {
    /// The server-assigned id, absent on a script not yet created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The script's user-facing name.
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        deserialize_with = "deserialize_null_as_default"
    )]
    pub name: String,
    /// The id of the blob holding the script's Sieve source.
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub blob_id: Id,
    /// True if this is the account's currently active script.
    #[serde(default)]
    pub is_active: bool,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScript {
    /// Create a script with a name and the blob id of its source, not yet active.
    pub fn new(name: impl Into<String>, blob_id: impl Into<Id>) -> Self {
        Self {
            id: None,
            name: name.into(),
            blob_id: blob_id.into(),
            is_active: false,
            extra: BTreeMap::new(),
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the script's user-facing name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set whether this is the account's currently active script.
    pub fn is_active(mut self, is_active: bool) -> Self {
        self.is_active = is_active;
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `SieveScript/set` arguments (RFC 9661 §2.4).
/// Extends `/set` with `onSuccessActivateScript` to activate or deactivate scripts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SieveScriptSetRequest {
    /// The underlying `/set` arguments (ids to create, update and destroy).
    #[serde(flatten)]
    pub set: SetRequest<SieveScript>,
    /// Tracks property presence to distinguish key absence from JSON `null`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub on_success_activate_script: Option<Value>,
    /// If true, deactivate the account's active script once the `/set` succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_deactivate_script: Option<bool>,
}

fn deserialize_present<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

impl SieveScriptSetRequest {
    /// Create a request from plain `/set` arguments, with no activation side effect.
    pub fn new(set: SetRequest<SieveScript>) -> Self {
        Self {
            set,
            on_success_activate_script: None,
            on_success_deactivate_script: None,
        }
    }

    /// Activate the given script once the `/set` succeeds.
    pub fn activating(mut self, id: impl Into<Id>) -> Self {
        self.on_success_activate_script = Some(Value::String(id.into().to_string()));
        self
    }

    /// Deactivate the account's active script once the `/set` succeeds.
    pub fn deactivating(mut self) -> Self {
        self.on_success_activate_script = Some(Value::Null);
        self.on_success_deactivate_script = Some(true);
        self
    }

    /// Set whether the account's active script is deactivated once the `/set` succeeds.
    pub fn with_deactivate_script(mut self, deactivate: bool) -> Self {
        self.on_success_deactivate_script = Some(deactivate);
        self
    }
}

/// `SieveScript/query` filter (RFC 9661 §2.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SieveScriptQueryFilter {
    /// Match scripts whose name equals this value exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Match scripts whose active state equals this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
    /// Extra filter conditions the server supports that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptQueryFilter {
    /// Create a filter that matches every script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Match scripts whose name equals this value exactly.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Match scripts whose active state equals this value.
    pub fn with_is_active(mut self, is_active: bool) -> Self {
        self.is_active = Some(is_active);
        self
    }

    /// Set extra filter conditions beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `SieveScript/validate` arguments (RFC 9661 §2.6).
/// Requires `accountId` and the `blobId` of the uploaded script content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SieveScriptValidateRequest {
    /// The account to validate the script against.
    pub account_id: Id,
    /// The id of the blob holding the uploaded Sieve source to validate.
    pub blob_id: Id,
    /// Extra arguments the server supports that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptValidateRequest {
    /// Create a request to validate the given blob's Sieve source for an account.
    pub fn new(account_id: impl Into<Id>, blob_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            blob_id: blob_id.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Set extra arguments beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `SieveScript/validate` response (RFC 9661 §2.6).
/// Contains `accountId` and an optional `error` detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SieveScriptValidateResponse {
    /// The account the script was validated against.
    pub account_id: Id,
    /// The validation failure, absent if the script is valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SetError>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptValidateResponse {
    /// Create a response reporting that the script is valid.
    pub fn valid(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            error: None,
            extra: BTreeMap::new(),
        }
    }

    /// Create a response reporting that the script is invalid, with the given error.
    pub fn invalid(account_id: impl Into<Id>, error: SetError) -> Self {
        Self {
            account_id: account_id.into(),
            error: Some(error),
            extra: BTreeMap::new(),
        }
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Returns true if `error` is None.
    pub fn is_valid(&self) -> bool {
        self.error.is_none()
    }
}

/// The `SetError` types defined for `SieveScript/set` (RFC 9661 §2.4).
pub mod sieve_set_error {
    /// The Sieve script does not parse or fails the server's validation.
    pub const INVALID_SIEVE: &str = "invalidSieve";
    /// The script is the account's active script and cannot be destroyed.
    pub const SIEVE_IS_ACTIVE: &str = "sieveIsActive";
}
