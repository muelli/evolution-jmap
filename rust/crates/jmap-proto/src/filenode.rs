// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP File Storage extension (draft-ietf-jmap-filenode).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;
use crate::state::UtcDate;

/// A file, directory, or symbolic link in a JMAP file storage hierarchy
/// (draft-ietf-jmap-filenode §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    pub node_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<FileNodeRights>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNode {
    pub fn new(id: impl Into<Id>, name: impl Into<String>, node_type: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            parent_id: None,
            name: name.into(),
            blob_id: None,
            size: None,
            node_type: node_type.into(),
            node_role: None,
            created: None,
            modified: None,
            executable: None,
            my_rights: None,
            sha256: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_parent_id(mut self, parent_id: impl Into<Id>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_node_role(mut self, node_role: impl Into<String>) -> Self {
        self.node_role = Some(node_role.into());
        self
    }

    pub fn with_created(mut self, created: impl Into<UtcDate>) -> Self {
        self.created = Some(created.into());
        self
    }

    pub fn with_modified(mut self, modified: impl Into<UtcDate>) -> Self {
        self.modified = Some(modified.into());
        self
    }

    pub fn with_executable(mut self, executable: bool) -> Self {
        self.executable = Some(executable);
        self
    }

    pub fn with_my_rights(mut self, my_rights: FileNodeRights) -> Self {
        self.my_rights = Some(my_rights);
        self
    }

    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Access rights for a `FileNode` (draft-ietf-jmap-filenode §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileNodeRights {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_admin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_modify_content: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNodeRights {
    pub fn all() -> Self {
        Self {
            may_read: Some(true),
            may_write: Some(true),
            may_admin: Some(true),
            may_modify_content: Some(true),
            extra: BTreeMap::new(),
        }
    }

    pub fn read_only() -> Self {
        Self {
            may_read: Some(true),
            may_write: Some(false),
            may_admin: Some(false),
            may_modify_content: Some(false),
            extra: BTreeMap::new(),
        }
    }

    pub fn is_writable(&self) -> bool {
        self.may_write.unwrap_or(false)
    }

    pub fn with_may_read(mut self, may_read: bool) -> Self {
        self.may_read = Some(may_read);
        self
    }

    pub fn with_may_write(mut self, may_write: bool) -> Self {
        self.may_write = Some(may_write);
        self
    }

    pub fn with_may_admin(mut self, may_admin: bool) -> Self {
        self.may_admin = Some(may_admin);
        self
    }

    pub fn with_may_modify_content(mut self, may_modify_content: bool) -> Self {
        self.may_modify_content = Some(may_modify_content);
        self
    }
}

/// JMAP FileNode account capability properties (draft-ietf-jmap-filenode §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileNodeCapability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_node_depth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_file_node_name: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_node_query_sort_options: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNodeCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_file_node_depth(mut self, depth: u64) -> Self {
        self.max_file_node_depth = Some(depth);
        self
    }

    pub fn with_max_size_file_node_name(mut self, size: u64) -> Self {
        self.max_size_file_node_name = Some(size);
        self
    }

    pub fn with_file_node_query_sort_options(
        mut self,
        options: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.file_node_query_sort_options = options.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Filter condition for `FileNode/query` (draft-ietf-jmap-filenode §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileNodeQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descendant_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_parent_id: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_executable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_blob: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNodeQueryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_parent_id(mut self, parent_id: impl Into<Id>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn with_descendant_id(mut self, descendant_id: impl Into<Id>) -> Self {
        self.descendant_id = Some(descendant_id.into());
        self
    }

    pub fn with_has_parent_id(mut self, has_parent_id: bool) -> Self {
        self.has_parent_id = Some(has_parent_id);
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_node_type(mut self, node_type: impl Into<String>) -> Self {
        self.node_type = Some(node_type.into());
        self
    }

    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    pub fn with_is_executable(mut self, is_executable: bool) -> Self {
        self.is_executable = Some(is_executable);
        self
    }

    pub fn with_has_blob(mut self, has_blob: bool) -> Self {
        self.has_blob = Some(has_blob);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard FileNode nodeType values (draft-ietf-jmap-filenode §2).
pub mod node_type {
    pub const FILE: &str = "file";
    pub const DIRECTORY: &str = "directory";
    pub const SYMLINK: &str = "symlink";
    pub const OTHER: &str = "other";
}

/// Standard FileNode nodeRole values (draft-ietf-jmap-filenode §2).
pub mod node_role {
    pub const ROOT: &str = "root";
    pub const HOME: &str = "home";
    pub const TRASH: &str = "trash";
    pub const DOCUMENTS: &str = "documents";
    pub const PICTURES: &str = "pictures";
    pub const VIDEOS: &str = "videos";
    pub const MUSIC: &str = "music";
    pub const DOWNLOADS: &str = "downloads";
}

/// Standard FileNode SetError types (draft-ietf-jmap-filenode §3.2).
pub mod filenode_set_error {
    pub const NODE_HAS_CHILDREN: &str = "nodeHasChildren";
    pub const ALREADY_EXISTS: &str = "alreadyExists";
    pub const INVALID_NODE_TYPE: &str = "invalidNodeType";
}
