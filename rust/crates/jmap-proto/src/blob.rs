// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Blob Management (RFC 9404): `Blob/get`, `Blob/upload`, `Blob/lookup`,
//! capabilities, and related request/response envelopes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SetError;
use crate::id::Id;

/// Blob capability properties (RFC 9404 §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BlobCapability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_blob_set: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_data_sources: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_type_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_digest_algorithms: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_source: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_target: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_size_blob_set(mut self, max: u64) -> Self {
        self.max_size_blob_set = Some(max);
        self
    }

    pub fn with_max_data_sources(mut self, max: u64) -> Self {
        self.max_data_sources = Some(max);
        self
    }

    pub fn with_supported_type_names(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.supported_type_names = Some(names.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_supported_digest_algorithms(
        mut self,
        algs: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.supported_digest_algorithms = Some(algs.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_max_size_source(mut self, max: u64) -> Self {
        self.max_size_source = Some(max);
        self
    }

    pub fn with_max_size_target(mut self, max: u64) -> Self {
        self.max_size_target = Some(max);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

impl crate::session::Account {
    /// This account's blob capability object (RFC 9404 §1.1), typed.
    pub fn blob_capability(&self) -> Option<BlobCapability> {
        let val = self
            .account_capabilities
            .get(crate::session::CAPABILITY_BLOB)?;
        serde_json::from_value(val.clone()).ok()
    }
}

/// A source of octets for an uploaded blob (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSource {
    #[serde(
        rename = "data:asText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_text: Option<String>,
    #[serde(
        rename = "data:asBase64",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl DataSource {
    pub fn as_text(text: impl Into<String>) -> Self {
        Self {
            data_as_text: Some(text.into()),
            data_as_base64: None,
            blob_id: None,
            offset: None,
            length: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn as_base64(base64: impl Into<String>) -> Self {
        Self {
            data_as_text: None,
            data_as_base64: Some(base64.into()),
            blob_id: None,
            offset: None,
            length: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn from_blob_id(blob_id: impl Into<Id>) -> Self {
        Self {
            data_as_text: None,
            data_as_base64: None,
            blob_id: Some(blob_id.into()),
            offset: None,
            length: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// An object to upload in `Blob/upload` (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UploadBlob {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<DataSource>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl UploadBlob {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(text: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self::new()
            .with_data_source(DataSource::as_text(text))
            .with_content_type(content_type)
    }

    pub fn from_base64(base64: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self::new()
            .with_data_source(DataSource::as_base64(base64))
            .with_content_type(content_type)
    }

    pub fn with_data(mut self, data: impl IntoIterator<Item = DataSource>) -> Self {
        self.data = data.into_iter().collect();
        self
    }

    pub fn with_data_source(mut self, source: DataSource) -> Self {
        self.data.push(source);
        self
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Result of an uploaded blob in `Blob/upload` (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadBlobResult {
    pub id: Id,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub size: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl UploadBlobResult {
    pub fn new(id: impl Into<Id>, size: u64) -> Self {
        Self {
            id: id.into(),
            content_type: None,
            size,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Blob/upload` arguments (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobUploadRequest {
    pub account_id: Id,
    pub create: BTreeMap<String, UploadBlob>,
}

impl BlobUploadRequest {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            create: BTreeMap::new(),
        }
    }

    pub fn create_blob(mut self, creation_id: impl Into<String>, blob: UploadBlob) -> Self {
        self.create.insert(creation_id.into(), blob);
        self
    }
}

/// `Blob/upload` response (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobUploadResponse {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, UploadBlobResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, SetError>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobUploadResponse {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            created: None,
            not_created: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_created(mut self, created: BTreeMap<String, UploadBlobResult>) -> Self {
        self.created = Some(created);
        self
    }

    pub fn with_not_created(mut self, not_created: BTreeMap<String, SetError>) -> Self {
        self.not_created = Some(not_created);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Metadata and data for a blob (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobInfo {
    pub id: Id,
    #[serde(
        rename = "data:asText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_text: Option<String>,
    #[serde(
        rename = "data:asBase64",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobInfo {
    pub fn new(id: impl Into<Id>, size: u64) -> Self {
        Self {
            id: id.into(),
            data_as_text: None,
            data_as_base64: None,
            data: None,
            content_type: None,
            size: Some(size),
            extra: BTreeMap::new(),
        }
    }

    pub fn from_id(id: impl Into<Id>) -> Self {
        Self {
            id: id.into(),
            data_as_text: None,
            data_as_base64: None,
            data: None,
            content_type: None,
            size: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_data_as_text(mut self, text: impl Into<String>) -> Self {
        self.data_as_text = Some(text.into());
        self
    }

    pub fn with_data_as_base64(mut self, base64: impl Into<String>) -> Self {
        self.data_as_base64 = Some(base64.into());
        self
    }

    pub fn with_data(mut self, data: impl Into<String>) -> Self {
        self.data = Some(data.into());
        self
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    pub fn with_digest(mut self, alg: impl AsRef<str>, digest: impl Into<String>) -> Self {
        self.extra.insert(
            format!("digest:{}", alg.as_ref()),
            Value::String(digest.into()),
        );
        self
    }

    pub fn digest(&self, alg: &str) -> Option<&str> {
        self.extra
            .get(&format!("digest:{alg}"))
            .and_then(|v| v.as_str())
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Blob/get` arguments (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobGetRequest {
    pub account_id: Id,
    #[serde(alias = "blobIds")]
    pub ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

impl BlobGetRequest {
    pub fn new(account_id: impl Into<Id>, ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        Self {
            account_id: account_id.into(),
            ids: ids.into_iter().map(Into::into).collect(),
            properties: None,
            offset: None,
            length: None,
        }
    }

    pub fn with_properties(
        mut self,
        properties: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }
}

/// `Blob/get` response (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobGetResponse {
    pub account_id: Id,
    #[serde(default)]
    pub list: Vec<BlobInfo>,
    #[serde(default)]
    pub not_found: Vec<Id>,
}

impl BlobGetResponse {
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = BlobInfo>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
            not_found: Vec::new(),
        }
    }

    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = not_found.into_iter().map(Into::into).collect();
        self
    }
}

/// `Blob/lookup` arguments (RFC 9404 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobLookupRequest {
    pub account_id: Id,
    pub type_names: Vec<String>,
    pub ids: Vec<Id>,
}

impl BlobLookupRequest {
    pub fn new(
        account_id: impl Into<Id>,
        type_names: impl IntoIterator<Item = impl Into<String>>,
        ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            type_names: type_names.into_iter().map(Into::into).collect(),
            ids: ids.into_iter().map(Into::into).collect(),
        }
    }
}

/// A matched blob lookup item (RFC 9404 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobLookupMatch {
    pub id: Id,
    #[serde(default)]
    pub matched_ids: BTreeMap<String, Vec<Id>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobLookupMatch {
    pub fn new(id: impl Into<Id>) -> Self {
        Self {
            id: id.into(),
            matched_ids: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    pub fn with_matched_ids(mut self, matched_ids: BTreeMap<String, Vec<Id>>) -> Self {
        self.matched_ids = matched_ids;
        self
    }

    pub fn with_type_matched_ids(
        mut self,
        type_name: impl Into<String>,
        ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.matched_ids
            .insert(type_name.into(), ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Blob/lookup` response (RFC 9404 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobLookupResponse {
    pub account_id: Id,
    #[serde(default)]
    pub list: Vec<BlobLookupMatch>,
    #[serde(default)]
    pub not_found: Vec<Id>,
}

impl BlobLookupResponse {
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = BlobLookupMatch>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
            not_found: Vec::new(),
        }
    }

    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = not_found.into_iter().map(Into::into).collect();
        self
    }
}

/// The `SetError` types added for blob management (RFC 9404 §4).
pub mod blob_set_error {
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    pub const TOO_LARGE: &str = "tooLarge";
    pub const MAX_DATA_SOURCES: &str = "maxDataSources";
}
