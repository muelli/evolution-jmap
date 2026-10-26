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
#[non_exhaustive]
pub struct BlobCapability {
    /// The largest `BlobSet` object the server accepts, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_blob_set: Option<u64>,
    /// The largest number of `DataSource` objects in one `Blob/upload` call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_data_sources: Option<u64>,
    /// The content types that `Blob/lookup` can search, or `None` for all types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_type_names: Option<Vec<String>>,
    /// The digest algorithm names `Blob/get` accepts in its `digest:` properties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_digest_algorithms: Option<Vec<String>>,
    /// The largest size, in bytes, of a blob a `DataSource` can reference by range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_source: Option<u64>,
    /// The largest size, in bytes, of a blob built by concatenating data sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_target: Option<u64>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobCapability {
    /// Create a `BlobCapability` with every field unset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the largest accepted `BlobSet` object size.
    pub fn with_max_size_blob_set(mut self, max: u64) -> Self {
        self.max_size_blob_set = Some(max);
        self
    }

    /// Set the largest accepted number of data sources per upload.
    pub fn with_max_data_sources(mut self, max: u64) -> Self {
        self.max_data_sources = Some(max);
        self
    }

    /// Set the content types `Blob/lookup` can search.
    pub fn with_supported_type_names(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.supported_type_names = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// Set the digest algorithm names `Blob/get` accepts.
    pub fn with_supported_digest_algorithms(
        mut self,
        algs: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.supported_digest_algorithms = Some(algs.into_iter().map(Into::into).collect());
        self
    }

    /// Set the largest size of a blob a data source can reference by range.
    pub fn with_max_size_source(mut self, max: u64) -> Self {
        self.max_size_source = Some(max);
        self
    }

    /// Set the largest size of a blob built from concatenated data sources.
    pub fn with_max_size_target(mut self, max: u64) -> Self {
        self.max_size_target = Some(max);
        self
    }

    /// Set the draft-defined properties this type does not name yet.
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
#[non_exhaustive]
pub struct DataSource {
    /// Literal text content, for a source built from inline text.
    #[serde(
        rename = "data:asText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_text: Option<String>,
    /// Base64-encoded content, for a source built from inline binary data.
    #[serde(
        rename = "data:asBase64",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_base64: Option<String>,
    /// The id of an existing blob to copy from, or a range of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    /// The byte offset into `blob_id` to start copying from, default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// The number of bytes to copy from `blob_id`, default to its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl DataSource {
    /// Create a data source from literal text.
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

    /// Create a data source from base64-encoded content.
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

    /// Create a data source that copies an existing blob by id.
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

    /// Set the byte offset to start copying from.
    pub fn with_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set the number of bytes to copy.
    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// An object to upload in `Blob/upload` (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UploadBlob {
    /// The data sources to concatenate into the new blob, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<DataSource>,
    /// The MIME type to store with the new blob.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl UploadBlob {
    /// Create an `UploadBlob` with no data sources and no content type.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an `UploadBlob` from a single text data source.
    pub fn from_text(text: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self::new()
            .with_data_source(DataSource::as_text(text))
            .with_content_type(content_type)
    }

    /// Create an `UploadBlob` from a single base64 data source.
    pub fn from_base64(base64: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self::new()
            .with_data_source(DataSource::as_base64(base64))
            .with_content_type(content_type)
    }

    /// Replace the full list of data sources.
    pub fn with_data(mut self, data: impl IntoIterator<Item = DataSource>) -> Self {
        self.data = data.into_iter().collect();
        self
    }

    /// Append one data source to the list.
    pub fn with_data_source(mut self, source: DataSource) -> Self {
        self.data.push(source);
        self
    }

    /// Set the MIME type to store with the new blob.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Result of an uploaded blob in `Blob/upload` (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UploadBlobResult {
    /// The id of the newly created blob.
    pub id: Id,
    /// The MIME type the server stored for the blob, if it kept one.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The size of the new blob in bytes.
    pub size: u64,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl UploadBlobResult {
    /// Create a result for a blob with the given id and size.
    pub fn new(id: impl Into<Id>, size: u64) -> Self {
        Self {
            id: id.into(),
            content_type: None,
            size,
            extra: BTreeMap::new(),
        }
    }

    /// Set the MIME type the server stored for the blob.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Blob/upload` arguments (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobUploadRequest {
    /// The id of the account to upload the blobs into.
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub account_id: Id,
    /// The blobs to create, keyed by a client-chosen creation id.
    pub create: BTreeMap<String, UploadBlob>,
}

impl BlobUploadRequest {
    /// Create a request for the given account with no blobs to create yet.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            create: BTreeMap::new(),
        }
    }

    /// Add one blob to create under the given creation id.
    pub fn create_blob(mut self, creation_id: impl Into<String>, blob: UploadBlob) -> Self {
        self.create.insert(creation_id.into(), blob);
        self
    }
}

/// `Blob/upload` response (RFC 9404 §2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobUploadResponse {
    /// The id of the account the blobs were uploaded into.
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub account_id: Id,
    /// The account's blob state before this upload, if the server tracks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<String>,
    /// The account's blob state after this upload, if the server tracks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_state: Option<String>,
    /// The results for each blob created, keyed by its creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, UploadBlobResult>>,
    /// The errors for each blob that failed to create, keyed by its creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, SetError>>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobUploadResponse {
    /// Create a response for the given account with no results yet.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            old_state: None,
            new_state: None,
            created: None,
            not_created: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the results for blobs that were created.
    pub fn with_created(mut self, created: BTreeMap<String, UploadBlobResult>) -> Self {
        self.created = Some(created);
        self
    }

    /// Set the errors for blobs that failed to create.
    pub fn with_not_created(mut self, not_created: BTreeMap<String, SetError>) -> Self {
        self.not_created = Some(not_created);
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Metadata and data for a blob (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobInfo {
    /// The id of the blob this entry describes.
    pub id: Id,
    /// The requested byte range, decoded as text, if the request asked for it.
    #[serde(
        rename = "data:asText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_text: Option<String>,
    /// The requested byte range, base64-encoded, if the request asked for it.
    #[serde(
        rename = "data:asBase64",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_as_base64: Option<String>,
    /// A requested `data` property value, for a property this type does not name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The MIME type the server has stored for this blob.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The full blob size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Whether `data:asText` could not decode the range as valid UTF-8 text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_encoding_problem: Option<bool>,
    /// Whether the returned range stopped short of the requested length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_truncated: Option<bool>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobInfo {
    /// Create a `BlobInfo` for the given id and size, all other fields unset.
    pub fn new(id: impl Into<Id>, size: u64) -> Self {
        Self {
            id: id.into(),
            data_as_text: None,
            data_as_base64: None,
            data: None,
            content_type: None,
            size: Some(size),
            is_encoding_problem: None,
            is_truncated: None,
            extra: BTreeMap::new(),
        }
    }

    /// Create a `BlobInfo` for the given id, all other fields unset.
    pub fn from_id(id: impl Into<Id>) -> Self {
        Self {
            id: id.into(),
            data_as_text: None,
            data_as_base64: None,
            data: None,
            content_type: None,
            size: None,
            is_encoding_problem: None,
            is_truncated: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set whether the text data could not be decoded as valid UTF-8.
    pub fn with_is_encoding_problem(mut self, problem: bool) -> Self {
        self.is_encoding_problem = Some(problem);
        self
    }

    /// Set whether the returned range stopped short of the requested length.
    pub fn with_is_truncated(mut self, truncated: bool) -> Self {
        self.is_truncated = Some(truncated);
        self
    }

    /// Set the full blob size in bytes.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the requested range, decoded as text.
    pub fn with_data_as_text(mut self, text: impl Into<String>) -> Self {
        self.data_as_text = Some(text.into());
        self
    }

    /// Set the requested range, base64-encoded.
    pub fn with_data_as_base64(mut self, base64: impl Into<String>) -> Self {
        self.data_as_base64 = Some(base64.into());
        self
    }

    /// Set the `data` property value.
    pub fn with_data(mut self, data: impl Into<String>) -> Self {
        self.data = Some(data.into());
        self
    }

    /// Set the MIME type stored for this blob.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the digest value for the given algorithm name.
    pub fn with_digest(mut self, alg: impl AsRef<str>, digest: impl Into<String>) -> Self {
        self.extra.insert(
            format!("digest:{}", alg.as_ref()),
            Value::String(digest.into()),
        );
        self
    }

    /// Read back the digest value for the given algorithm name, if present.
    pub fn digest(&self, alg: &str) -> Option<&str> {
        self.extra
            .get(&format!("digest:{alg}"))
            .and_then(|v| v.as_str())
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Blob/get` arguments (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobGetRequest {
    /// The id of the account the blobs belong to.
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub account_id: Id,
    /// The ids of the blobs to fetch.
    #[serde(alias = "blobIds", default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<Id>,
    /// A reference to the ids, used instead of `ids` in a result-chained call.
    #[serde(
        rename = "#ids",
        alias = "#blobIds",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ids_ref: Option<crate::request::ResultReference>,
    /// The `BlobInfo` properties to return, or `None` for the default set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    /// The byte offset to start reading from, default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// The number of bytes to read, default to the blob's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

impl BlobGetRequest {
    /// Create a request for the given account and blob ids.
    pub fn new(account_id: impl Into<Id>, ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        Self {
            account_id: account_id.into(),
            ids: ids.into_iter().map(Into::into).collect(),
            ids_ref: None,
            properties: None,
            offset: None,
            length: None,
        }
    }

    /// Fetch the ids from a prior method call's result instead of a literal list.
    pub fn with_ids_ref(mut self, ids_ref: crate::request::ResultReference) -> Self {
        self.ids_ref = Some(ids_ref);
        self
    }

    /// Set the `BlobInfo` properties to return.
    pub fn with_properties(
        mut self,
        properties: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }

    /// Set the byte offset to start reading from.
    pub fn with_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set the number of bytes to read.
    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }
}

/// `Blob/get` response (RFC 9404 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobGetResponse {
    /// The id of the account the blobs belong to.
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub account_id: Id,
    /// The blob info for each id that was found.
    #[serde(default)]
    pub list: Vec<BlobInfo>,
    /// The requested ids the server did not find.
    #[serde(default)]
    pub not_found: Vec<Id>,
}

impl BlobGetResponse {
    /// Create a response for the given account with the found blob info.
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = BlobInfo>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
            not_found: Vec::new(),
        }
    }

    /// Set the ids the server did not find.
    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = not_found.into_iter().map(Into::into).collect();
        self
    }
}

/// `Blob/lookup` arguments (RFC 9404 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobLookupRequest {
    /// The id of the account the blobs belong to.
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub account_id: Id,
    /// The object type names to search for a match against each blob id.
    pub type_names: Vec<String>,
    /// The blob ids to look up.
    pub ids: Vec<Id>,
}

impl BlobLookupRequest {
    /// Create a request for the given account, type names and blob ids.
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
#[non_exhaustive]
pub struct BlobLookupMatch {
    /// The blob id this match describes.
    pub id: Id,
    /// The matching object ids found for this blob, keyed by type name.
    #[serde(default)]
    pub matched_ids: BTreeMap<String, Vec<Id>>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BlobLookupMatch {
    /// Create a match for the given blob id with no matching ids yet.
    pub fn new(id: impl Into<Id>) -> Self {
        Self {
            id: id.into(),
            matched_ids: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Replace the full map of matching ids, keyed by type name.
    pub fn with_matched_ids(mut self, matched_ids: BTreeMap<String, Vec<Id>>) -> Self {
        self.matched_ids = matched_ids;
        self
    }

    /// Set the matching ids for one type name.
    pub fn with_type_matched_ids(
        mut self,
        type_name: impl Into<String>,
        ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.matched_ids
            .insert(type_name.into(), ids.into_iter().map(Into::into).collect());
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Blob/lookup` response (RFC 9404 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BlobLookupResponse {
    /// The id of the account the blobs belong to.
    #[serde(default, skip_serializing_if = "Id::is_empty")]
    pub account_id: Id,
    /// The matches for each blob id that was found.
    #[serde(default)]
    pub list: Vec<BlobLookupMatch>,
    /// The requested ids the server did not find.
    #[serde(default)]
    pub not_found: Vec<Id>,
}

impl BlobLookupResponse {
    /// Create a response for the given account with the found matches.
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = BlobLookupMatch>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
            not_found: Vec::new(),
        }
    }

    /// Set the ids the server did not find.
    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = not_found.into_iter().map(Into::into).collect();
        self
    }
}

/// The `SetError` types added for blob management (RFC 9404 §4).
pub mod blob_set_error {
    /// A referenced blob id does not exist, or is not visible to this account.
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    /// The resulting blob would exceed a server-defined size limit.
    pub const TOO_LARGE: &str = "tooLarge";
    /// The upload used more data sources than the server allows.
    pub const MAX_DATA_SOURCES: &str = "maxDataSources";
}
