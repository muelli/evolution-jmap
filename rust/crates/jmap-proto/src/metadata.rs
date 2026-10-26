// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Object Metadata extension (draft-ietf-jmap-metadata).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JMAP Object Metadata account capability properties (draft-ietf-jmap-metadata §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MetadataCapability {
    /// Metadata support for each data type the server reports, keyed by its name.
    #[serde(default)]
    pub data_types: BTreeMap<String, DataTypeMetadataInfo>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MetadataCapability {
    /// Create an empty capability with no data types.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace the metadata info for one data type.
    pub fn with_data_type(
        mut self,
        data_type: impl Into<String>,
        info: DataTypeMetadataInfo,
    ) -> Self {
        self.data_types.insert(data_type.into(), info);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Metadata capabilities for a specific JMAP data type (draft-ietf-jmap-metadata §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DataTypeMetadataInfo {
    /// Namespaces the server accepts for this data type's metadata paths.
    #[serde(default)]
    pub namespaces: Vec<String>,
    /// True if the server accepts vendor-specific namespaces for this data type.
    #[serde(default)]
    pub supports_vendor_namespaces: bool,
    /// True if the server supports private (per-user) metadata for this data type.
    #[serde(default)]
    pub supports_private: bool,
    /// The deepest metadata path the server accepts, if it limits depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u64>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl DataTypeMetadataInfo {
    /// Create an empty info with no namespaces and no limits set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the namespace list with the given namespaces.
    pub fn with_namespaces(
        mut self,
        namespaces: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.namespaces = namespaces.into_iter().map(Into::into).collect();
        self
    }

    /// Append one namespace to the namespace list.
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespaces.push(namespace.into());
        self
    }

    /// Set whether the server accepts vendor-specific namespaces.
    pub fn supports_vendor_namespaces(mut self, supports: bool) -> Self {
        self.supports_vendor_namespaces = supports;
        self
    }

    /// Set whether the server supports private metadata for this data type.
    pub fn supports_private(mut self, supports: bool) -> Self {
        self.supports_private = supports;
        self
    }

    /// Set the deepest metadata path the server accepts.
    pub fn with_max_depth(mut self, max_depth: u64) -> Self {
        self.max_depth = Some(max_depth);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// A text filter condition on metadata properties (draft-ietf-jmap-metadata §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MetadataTextFilter {
    /// The metadata path to match against.
    pub path: String,
    /// The text to look for at that path.
    pub text: String,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MetadataTextFilter {
    /// Create a filter matching `text` at `path`.
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Filter condition for queries matching object metadata (draft-ietf-jmap-metadata §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MetadataFilterCondition {
    /// Match objects that have metadata at this path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_exists: Option<String>,
    /// Match objects whose metadata at a path contains this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_text_contains: Option<MetadataTextFilter>,
    /// Match objects whose metadata at a path equals this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_text_equals: Option<MetadataTextFilter>,
    /// Match objects that have private metadata at this path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_metadata_exists: Option<String>,
    /// Match objects whose private metadata at a path contains this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_metadata_text_contains: Option<MetadataTextFilter>,
    /// Match objects whose private metadata at a path equals this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_metadata_text_equals: Option<MetadataTextFilter>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MetadataFilterCondition {
    /// Create an empty filter condition with no checks set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Match objects that have metadata at `path`.
    pub fn with_metadata_exists(mut self, path: impl Into<String>) -> Self {
        self.metadata_exists = Some(path.into());
        self
    }

    /// Match objects whose metadata at `path` contains `text`.
    pub fn with_metadata_text_contains(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.metadata_text_contains = Some(MetadataTextFilter::new(path, text));
        self
    }

    /// Match objects whose metadata at `path` equals `text`.
    pub fn with_metadata_text_equals(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.metadata_text_equals = Some(MetadataTextFilter::new(path, text));
        self
    }

    /// Match objects that have private metadata at `path`.
    pub fn with_private_metadata_exists(mut self, path: impl Into<String>) -> Self {
        self.private_metadata_exists = Some(path.into());
        self
    }

    /// Match objects whose private metadata at `path` contains `text`.
    pub fn with_private_metadata_text_contains(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.private_metadata_text_contains = Some(MetadataTextFilter::new(path, text));
        self
    }

    /// Match objects whose private metadata at `path` equals `text`.
    pub fn with_private_metadata_text_equals(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.private_metadata_text_equals = Some(MetadataTextFilter::new(path, text));
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}
