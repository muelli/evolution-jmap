// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Object Metadata extension (draft-ietf-jmap-metadata).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JMAP Object Metadata account capability properties (draft-ietf-jmap-metadata §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataCapability {
    #[serde(default)]
    pub data_types: BTreeMap<String, DataTypeMetadataInfo>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MetadataCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_data_type(
        mut self,
        data_type: impl Into<String>,
        info: DataTypeMetadataInfo,
    ) -> Self {
        self.data_types.insert(data_type.into(), info);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Metadata capabilities for a specific JMAP data type (draft-ietf-jmap-metadata §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DataTypeMetadataInfo {
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub supports_vendor_namespaces: bool,
    #[serde(default)]
    pub supports_private: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl DataTypeMetadataInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_namespaces(
        mut self,
        namespaces: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.namespaces = namespaces.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespaces.push(namespace.into());
        self
    }

    pub fn supports_vendor_namespaces(mut self, supports: bool) -> Self {
        self.supports_vendor_namespaces = supports;
        self
    }

    pub fn supports_private(mut self, supports: bool) -> Self {
        self.supports_private = supports;
        self
    }

    pub fn with_max_depth(mut self, max_depth: u64) -> Self {
        self.max_depth = Some(max_depth);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// A text filter condition on metadata properties (draft-ietf-jmap-metadata §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataTextFilter {
    pub path: String,
    pub text: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MetadataTextFilter {
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
            extra: BTreeMap::new(),
        }
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Filter condition for queries matching object metadata (draft-ietf-jmap-metadata §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFilterCondition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_exists: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_text_contains: Option<MetadataTextFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_text_equals: Option<MetadataTextFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_metadata_exists: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_metadata_text_contains: Option<MetadataTextFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_metadata_text_equals: Option<MetadataTextFilter>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MetadataFilterCondition {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_metadata_exists(mut self, path: impl Into<String>) -> Self {
        self.metadata_exists = Some(path.into());
        self
    }

    pub fn with_metadata_text_contains(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.metadata_text_contains = Some(MetadataTextFilter::new(path, text));
        self
    }

    pub fn with_metadata_text_equals(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.metadata_text_equals = Some(MetadataTextFilter::new(path, text));
        self
    }

    pub fn with_private_metadata_exists(mut self, path: impl Into<String>) -> Self {
        self.private_metadata_exists = Some(path.into());
        self
    }

    pub fn with_private_metadata_text_contains(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.private_metadata_text_contains = Some(MetadataTextFilter::new(path, text));
        self
    }

    pub fn with_private_metadata_text_equals(
        mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.private_metadata_text_equals = Some(MetadataTextFilter::new(path, text));
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}
