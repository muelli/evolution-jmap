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

pub const CAPABILITY_SIEVE: &str = "urn:ietf:params:jmap:sieve";

/// Sieve capability properties (RFC 9661 §1.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SieveCapability {
    pub max_size_script: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_number_scripts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sieve_extensions: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveCapability {
    pub fn new(max_size_script: u64) -> Self {
        Self {
            max_size_script,
            max_number_scripts: None,
            implementation: None,
            sieve_extensions: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    pub fn with_max_number_scripts(mut self, max: u64) -> Self {
        self.max_number_scripts = Some(max);
        self
    }

    pub fn with_implementation(mut self, implementation: impl Into<String>) -> Self {
        self.implementation = Some(implementation.into());
        self
    }

    pub fn with_sieve_extensions(
        mut self,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.sieve_extensions = extensions.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_sieve_extension(mut self, extension: impl Into<String>) -> Self {
        self.sieve_extensions.push(extension.into());
        self
    }

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        deserialize_with = "deserialize_null_as_default"
    )]
    pub name: String,
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub blob_id: Id,
    #[serde(default)]
    pub is_active: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScript {
    pub fn new(name: impl Into<String>, blob_id: impl Into<Id>) -> Self {
        Self {
            id: None,
            name: name.into(),
            blob_id: blob_id.into(),
            is_active: false,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn is_active(mut self, is_active: bool) -> Self {
        self.is_active = is_active;
        self
    }

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
    #[serde(flatten)]
    pub set: SetRequest<SieveScript>,
    /// Tracks property presence to distinguish key absence from JSON `null`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub on_success_activate_script: Option<Value>,
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
    pub fn new(set: SetRequest<SieveScript>) -> Self {
        Self {
            set,
            on_success_activate_script: None,
            on_success_deactivate_script: None,
        }
    }

    pub fn activating(mut self, id: impl Into<Id>) -> Self {
        self.on_success_activate_script = Some(Value::String(id.into().to_string()));
        self
    }

    pub fn deactivating(mut self) -> Self {
        self.on_success_activate_script = Some(Value::Null);
        self.on_success_deactivate_script = Some(true);
        self
    }

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptQueryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_is_active(mut self, is_active: bool) -> Self {
        self.is_active = Some(is_active);
        self
    }

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
    pub account_id: Id,
    pub blob_id: Id,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptValidateRequest {
    pub fn new(account_id: impl Into<Id>, blob_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            blob_id: blob_id.into(),
            extra: BTreeMap::new(),
        }
    }

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
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SetError>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptValidateResponse {
    pub fn valid(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            error: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn invalid(account_id: impl Into<Id>, error: SetError) -> Self {
        Self {
            account_id: account_id.into(),
            error: Some(error),
            extra: BTreeMap::new(),
        }
    }

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
    pub const INVALID_SIEVE: &str = "invalidSieve";
    pub const SIEVE_IS_ACTIVE: &str = "sieveIsActive";
}
