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

/// `SieveScript/set` arguments: the standard `/set` shape plus the
/// `onSuccessActivateScript` extension (RFC 9661 §2.4). A script's `isActive`
/// is server-set and only ever changes through this argument: `null`
/// deactivates whatever script is currently active, a plain id activates
/// that script, and a `#`-prefixed value is a creation id from the same
/// call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SieveScriptSetRequest {
    #[serde(flatten)]
    pub set: SetRequest<SieveScript>,
    /// `serde`'s blanket `Option<T>` impl treats a JSON `null` the same as
    /// the key being absent, collapsing both to `None` — indistinguishable
    /// here, where they mean opposite things ("leave the active script
    /// alone" vs "deactivate it"). `deserialize_present` only runs when the
    /// key is there at all, `null` included, so it is the field's presence
    /// that this `Option` tracks, not whether its value is non-null.
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

/// `SieveScript/validate` arguments (RFC 9661 §2.6). The RFC names exactly
/// two arguments, `accountId` and `blobId`: script content must already be
/// uploaded as a blob (§2.2) before it can be validated, there is no raw
/// `content` or existing-script `id` shorthand on the wire. An earlier
/// version of this type invented both as convenience fields; real Stalwart
/// rejected them (confirmed live, 2026-10-10: a request with only `content`
/// came back `blobNotFound`, since it has no `blobId` at all). The
/// `jmap-client` crate's `Client::sieve_script_validate_content` is the
/// client-side equivalent: it uploads the content first, then validates the
/// resulting `blobId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// `SieveScript/validate` response (RFC 9661 §2.6). The RFC names exactly
/// two response arguments, `accountId` and `error` (a `SetError|null`);
/// there is no `isValid` property on the wire at all, validity is simply
/// whether `error` is null. Confirmed live against real Stalwart
/// (2026-10-10): a valid script answers `{"accountId":"b0","error":null}`
/// and an invalid one `{"accountId":"b0","error":{"type":"invalidScript",
/// "description":"..."}}`, neither with an `isValid` field. An earlier
/// version of this type declared `isValid` as a mandatory field, so it
/// failed to deserialize every real server response; [`Self::is_valid`]
/// replaces it as a derived accessor instead of a wire field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

    /// No wire field names this; it is simply whether `error` is null.
    pub fn is_valid(&self) -> bool {
        self.error.is_none()
    }
}

/// The two `SetError` types RFC 9661 §2.4 defines for `SieveScript/set`:
/// `invalidSieve` for a create or update whose content fails to parse, and
/// `sieveIsActive` for a destroy of the currently active script (it must be
/// deactivated in a separate call first). The RFC reuses three standard RFC
/// 8620 §5.3 errors rather than defining Sieve-specific ones for the other
/// documented failures: `alreadyExists` (duplicate name, with an
/// `existingId` property), `tooLarge` (over `maxSizeScript`) and `overQuota`
/// (over `maxNumberScripts` or storage) — see [`crate::error::set`].
/// Activating two scripts at once cannot arise: `onSuccessActivateScript`
/// names at most one id per call, so there is no `multipleActiveScripts`
/// error to report.
pub mod sieve_set_error {
    pub const INVALID_SIEVE: &str = "invalidSieve";
    pub const SIEVE_IS_ACTIVE: &str = "sieveIsActive";
}
