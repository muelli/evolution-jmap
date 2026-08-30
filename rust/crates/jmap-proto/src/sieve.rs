// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9265: JMAP for Sieve Scripts.
//!
//! Models the `SieveScript` data type, query filters, `SieveScript/validate`
//! method envelopes, capability object, and standard error codes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;

pub const CAPABILITY_SIEVE: &str = "urn:ietf:params:jmap:sieve";

/// Sieve capability properties (RFC 9265 §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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

/// A Sieve script object (RFC 9265 §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SieveScript {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: String,
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

    pub fn is_active(mut self, is_active: bool) -> Self {
        self.is_active = is_active;
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `SieveScript/query` filter (RFC 9265 §2.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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

/// `SieveScript/validate` arguments (RFC 9265 §2.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SieveScriptValidateRequest {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptValidateRequest {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            id: None,
            blob_id: None,
            content: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `SieveScript/validate` response (RFC 9265 §2.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SieveScriptValidateResponse {
    pub account_id: Id,
    pub is_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SieveScriptValidateError>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptValidateResponse {
    pub fn valid(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            is_valid: true,
            error: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn invalid(account_id: impl Into<Id>, error: SieveScriptValidateError) -> Self {
        Self {
            account_id: account_id.into(),
            is_valid: false,
            error: Some(error),
            extra: BTreeMap::new(),
        }
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Detailed syntax or semantic error in a Sieve script (RFC 9265 §2.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SieveScriptValidateError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SieveScriptValidateError {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_line_number(mut self, line_number: u64) -> Self {
        self.line_number = Some(line_number);
        self
    }

    pub fn with_column_number(mut self, column_number: u64) -> Self {
        self.column_number = Some(column_number);
        self
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// The `SetError` types RFC 9265 §2.3.2 adds for `SieveScript/set`.
pub mod sieve_set_error {
    pub const CANNOT_DELETE_ACTIVE_SCRIPT: &str = "cannotDeleteActiveScript";
    pub const SIEVE_IS_ACTIVE: &str = "sieveIsActive";
    pub const DUPLICATE_SCRIPT_NAME: &str = "duplicateScriptName";
    pub const INVALID_SIEVE: &str = "invalidSieve";
    pub const MAX_NUMBER_SCRIPTS_EXCEEDED: &str = "maxNumberScriptsExceeded";
    pub const MAX_SIZE_SCRIPT_EXCEEDED: &str = "maxSizeScriptExceeded";
    pub const MULTIPLE_ACTIVE_SCRIPTS: &str = "multipleActiveScripts";
}
