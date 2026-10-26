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
#[non_exhaustive]
pub struct FileNode {
    /// The id of this file node.
    pub id: Id,
    /// The id of the parent directory, or `None` at the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    /// The file or directory name, unique among its siblings.
    pub name: String,
    /// The id of the blob holding the file's content, for a file node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    /// The content size in bytes, for a file node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The node kind: one of the [`node_type`] constants.
    pub node_type: String,
    /// The node's standard role, if any: one of the [`node_role`] constants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_role: Option<String>,
    /// The time this node was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<UtcDate>,
    /// The time this node was last modified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<UtcDate>,
    /// Whether the file is executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
    /// The current user's rights on this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<FileNodeRights>,
    /// The SHA-256 digest of the file's content, as a hex string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNode {
    /// Create a `FileNode` with the given id, name and node type, all other fields unset.
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

    /// Set the parent directory id.
    pub fn with_parent_id(mut self, parent_id: impl Into<Id>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Set the content blob id.
    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    /// Set the content size in bytes.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the node's standard role.
    pub fn with_node_role(mut self, node_role: impl Into<String>) -> Self {
        self.node_role = Some(node_role.into());
        self
    }

    /// Set the creation time.
    pub fn with_created(mut self, created: impl Into<UtcDate>) -> Self {
        self.created = Some(created.into());
        self
    }

    /// Set the last-modified time.
    pub fn with_modified(mut self, modified: impl Into<UtcDate>) -> Self {
        self.modified = Some(modified.into());
        self
    }

    /// Set whether the file is executable.
    pub fn with_executable(mut self, executable: bool) -> Self {
        self.executable = Some(executable);
        self
    }

    /// Set the current user's rights on this node.
    pub fn with_my_rights(mut self, my_rights: FileNodeRights) -> Self {
        self.my_rights = Some(my_rights);
        self
    }

    /// Set the SHA-256 digest of the file's content, as a hex string.
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }

    /// Set the server- or draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Access rights for a `FileNode` (draft-ietf-jmap-filenode §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FileNodeRights {
    /// Whether the current user may read this node and its content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read: Option<bool>,
    /// Whether the current user may create, rename or delete children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_write: Option<bool>,
    /// Whether the current user may change this node's sharing rights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_admin: Option<bool>,
    /// Whether the current user may change this file's content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_modify_content: Option<bool>,
    /// Server- or draft-defined rights this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNodeRights {
    /// Build rights with every permission granted.
    pub fn all() -> Self {
        Self {
            may_read: Some(true),
            may_write: Some(true),
            may_admin: Some(true),
            may_modify_content: Some(true),
            extra: BTreeMap::new(),
        }
    }

    /// Build rights that grant only read access.
    pub fn read_only() -> Self {
        Self {
            may_read: Some(true),
            may_write: Some(false),
            may_admin: Some(false),
            may_modify_content: Some(false),
            extra: BTreeMap::new(),
        }
    }

    /// Report whether `may_write` is set and true.
    pub fn is_writable(&self) -> bool {
        self.may_write.unwrap_or(false)
    }

    /// Set whether the current user may read this node and its content.
    pub fn with_may_read(mut self, may_read: bool) -> Self {
        self.may_read = Some(may_read);
        self
    }

    /// Set whether the current user may create, rename or delete children.
    pub fn with_may_write(mut self, may_write: bool) -> Self {
        self.may_write = Some(may_write);
        self
    }

    /// Set whether the current user may change this node's sharing rights.
    pub fn with_may_admin(mut self, may_admin: bool) -> Self {
        self.may_admin = Some(may_admin);
        self
    }

    /// Set whether the current user may change this file's content.
    pub fn with_may_modify_content(mut self, may_modify_content: bool) -> Self {
        self.may_modify_content = Some(may_modify_content);
        self
    }
}

/// JMAP FileNode account capability properties (draft-ietf-jmap-filenode §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FileNodeCapability {
    /// The deepest directory nesting level the server allows, if limited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_node_depth: Option<u64>,
    /// The longest file or directory name the server allows, in UTF-8 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_file_node_name: Option<u64>,
    /// The property names the server accepts as `FileNode/query` sort keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_node_query_sort_options: Vec<String>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNodeCapability {
    /// Create a capability object with every field at its default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the deepest directory nesting level the server allows.
    pub fn with_max_file_node_depth(mut self, depth: u64) -> Self {
        self.max_file_node_depth = Some(depth);
        self
    }

    /// Set the longest file or directory name the server allows, in UTF-8 bytes.
    pub fn with_max_size_file_node_name(mut self, size: u64) -> Self {
        self.max_size_file_node_name = Some(size);
        self
    }

    /// Set the property names the server accepts as `FileNode/query` sort keys.
    pub fn with_file_node_query_sort_options(
        mut self,
        options: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.file_node_query_sort_options = options.into_iter().map(Into::into).collect();
        self
    }

    /// Set the server- or draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Filter condition for `FileNode/query` (draft-ietf-jmap-filenode §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FileNodeQueryFilter {
    /// Match only direct children of this directory id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    /// Match only nodes nested anywhere under this directory id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descendant_id: Option<Id>,
    /// Match only nodes that do, or do not, have a parent, per the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_parent_id: Option<bool>,
    /// Match only nodes with exactly this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Match only nodes of this [`node_type`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    /// Match only nodes with this standard [`node_role`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Match only nodes whose `executable` flag equals this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_executable: Option<bool>,
    /// Match only nodes that do, or do not, have a content blob, per the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_blob: Option<bool>,
    /// Server- or draft-defined filter conditions this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FileNodeQueryFilter {
    /// Create a filter that matches every node.
    pub fn new() -> Self {
        Self::default()
    }

    /// Match only direct children of this directory id.
    pub fn with_parent_id(mut self, parent_id: impl Into<Id>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Match only nodes nested anywhere under this directory id.
    pub fn with_descendant_id(mut self, descendant_id: impl Into<Id>) -> Self {
        self.descendant_id = Some(descendant_id.into());
        self
    }

    /// Match only nodes that do, or do not, have a parent, per the value.
    pub fn with_has_parent_id(mut self, has_parent_id: bool) -> Self {
        self.has_parent_id = Some(has_parent_id);
        self
    }

    /// Match only nodes with exactly this name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Match only nodes of this node type.
    pub fn with_node_type(mut self, node_type: impl Into<String>) -> Self {
        self.node_type = Some(node_type.into());
        self
    }

    /// Match only nodes with this standard role.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Match only nodes whose `executable` flag equals this value.
    pub fn with_is_executable(mut self, is_executable: bool) -> Self {
        self.is_executable = Some(is_executable);
        self
    }

    /// Match only nodes that do, or do not, have a content blob, per the value.
    pub fn with_has_blob(mut self, has_blob: bool) -> Self {
        self.has_blob = Some(has_blob);
        self
    }

    /// Set the server- or draft-defined filter conditions this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard FileNode nodeType values (draft-ietf-jmap-filenode §2).
pub mod node_type {
    /// A regular file.
    pub const FILE: &str = "file";
    /// A directory that may hold child nodes.
    pub const DIRECTORY: &str = "directory";
    /// A symbolic link to another node.
    pub const SYMLINK: &str = "symlink";
    /// A node kind the draft does not otherwise name.
    pub const OTHER: &str = "other";
}

/// Standard FileNode nodeRole values (draft-ietf-jmap-filenode §2).
pub mod node_role {
    /// The root directory of the file hierarchy.
    pub const ROOT: &str = "root";
    /// The current user's home directory.
    pub const HOME: &str = "home";
    /// The directory that holds deleted nodes.
    pub const TRASH: &str = "trash";
    /// The standard directory for documents.
    pub const DOCUMENTS: &str = "documents";
    /// The standard directory for pictures.
    pub const PICTURES: &str = "pictures";
    /// The standard directory for videos.
    pub const VIDEOS: &str = "videos";
    /// The standard directory for music.
    pub const MUSIC: &str = "music";
    /// The standard directory for downloads.
    pub const DOWNLOADS: &str = "downloads";
}

/// Standard FileNode SetError types (draft-ietf-jmap-filenode §3.2).
pub mod filenode_set_error {
    /// The node has children and cannot be deleted without `onDestroyRemoveChildren`.
    pub const NODE_HAS_CHILDREN: &str = "nodeHasChildren";
    /// A node with this name already exists under the same parent.
    pub const ALREADY_EXISTS: &str = "alreadyExists";
    /// The `nodeType` is not one the server accepts for a create or update.
    pub const INVALID_NODE_TYPE: &str = "invalidNodeType";
}
