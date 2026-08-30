// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Blob Management (RFC 9404): `Blob/get`, `Blob/upload`, capabilities,
//! and related request/response envelopes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SetError;
use crate::id::Id;
use crate::state::State;

/// Blob capability properties (RFC 9404 §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BlobCapability {
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

    pub fn with_max_size_source(mut self, max: u64) -> Self {
        self.max_size_source = Some(max);
        self
    }

    pub fn with_max_size_target(mut self, max: u64) -> Self {
        self.max_size_target = Some(max);
        self
    }
}

/// Metadata and optional data for a blob (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobInfo {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub size: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobInfo {
    pub fn new(id: impl Into<Id>, size: u64) -> Self {
        Self {
            id: id.into(),
            data: None,
            content_type: None,
            size,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_data(mut self, data: impl Into<String>) -> Self {
        self.data = Some(data.into());
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

/// `Blob/get` arguments (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobGetRequest {
    pub account_id: Id,
    pub blob_ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

impl BlobGetRequest {
    pub fn new(
        account_id: impl Into<Id>,
        blob_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            blob_ids: blob_ids.into_iter().map(Into::into).collect(),
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

/// An object to upload in `Blob/upload` (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UploadBlob {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl UploadBlob {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_data(mut self, data: impl Into<String>) -> Self {
        self.data = Some(data.into());
        self
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
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
    pub old_state: Option<State>,
    pub new_state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, UploadBlobResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, SetError>>,
}

impl BlobUploadResponse {
    pub fn new(account_id: impl Into<Id>, new_state: impl Into<State>) -> Self {
        Self {
            account_id: account_id.into(),
            old_state: None,
            new_state: new_state.into(),
            created: None,
            not_created: None,
        }
    }

    pub fn with_old_state(mut self, old_state: impl Into<State>) -> Self {
        self.old_state = Some(old_state.into());
        self
    }

    pub fn with_created(mut self, created: BTreeMap<String, UploadBlobResult>) -> Self {
        self.created = Some(created);
        self
    }

    pub fn with_not_created(mut self, not_created: BTreeMap<String, SetError>) -> Self {
        self.not_created = Some(not_created);
        self
    }
}

/// The `SetError` types added for blob management (RFC 9404 §4).
pub mod blob_set_error {
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    pub const TOO_LARGE: &str = "tooLarge";
}
