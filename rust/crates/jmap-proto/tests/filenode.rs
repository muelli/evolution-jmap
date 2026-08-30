// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::UtcDate;
use jmap_proto::filenode::{
    FileNode, FileNodeCapability, FileNodeQueryFilter, FileNodeRights, filenode_set_error,
    node_role, node_type,
};
use jmap_proto::request::ResultReference;
use jmap_proto::session::{CAPABILITY_FILENODE, CAPABILITY_REFPLUS, Session};
use serde_json::json;

#[test]
fn test_filenode_serialization_and_deserialization() {
    let now = UtcDate::new("2026-08-30T12:00:00Z");
    let rights = FileNodeRights::all();
    let node = FileNode::new("fn-123", "document.pdf", node_type::FILE)
        .with_parent_id("dir-root")
        .with_blob_id("blob-999")
        .with_size(1048576)
        .with_node_role(node_role::DOCUMENTS)
        .with_created(now.clone())
        .with_modified(now.clone())
        .with_executable(false)
        .with_my_rights(rights)
        .with_sha256("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");

    let val = serde_json::to_value(&node).unwrap();
    assert_eq!(val["id"], "fn-123");
    assert_eq!(val["parentId"], "dir-root");
    assert_eq!(val["name"], "document.pdf");
    assert_eq!(val["blobId"], "blob-999");
    assert_eq!(val["size"], 1048576);
    assert_eq!(val["nodeType"], "file");
    assert_eq!(val["nodeRole"], "documents");
    assert_eq!(val["created"], "2026-08-30T12:00:00Z");
    assert_eq!(val["modified"], "2026-08-30T12:00:00Z");
    assert_eq!(val["executable"], false);
    assert_eq!(val["myRights"]["mayRead"], true);
    assert_eq!(val["myRights"]["mayWrite"], true);
    assert_eq!(val["myRights"]["mayAdmin"], true);
    assert_eq!(val["myRights"]["mayModifyContent"], true);
    assert_eq!(
        val["sha256"],
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    let roundtripped: FileNode = serde_json::from_value(val).unwrap();
    assert_eq!(roundtripped, node);
}

#[test]
fn test_filenode_rights_helpers() {
    let all = FileNodeRights::all();
    assert!(all.is_writable());
    assert_eq!(all.may_read, Some(true));
    assert_eq!(all.may_write, Some(true));
    assert_eq!(all.may_admin, Some(true));
    assert_eq!(all.may_modify_content, Some(true));

    let ro = FileNodeRights::read_only();
    assert!(!ro.is_writable());
    assert_eq!(ro.may_read, Some(true));
    assert_eq!(ro.may_write, Some(false));
    assert_eq!(ro.may_admin, Some(false));
    assert_eq!(ro.may_modify_content, Some(false));
}

#[test]
fn test_filenode_query_filter_builders() {
    let filter = FileNodeQueryFilter::new()
        .with_parent_id("dir-root")
        .with_descendant_id("desc-1")
        .with_has_parent_id(true)
        .with_name("project")
        .with_node_type(node_type::DIRECTORY)
        .with_role(node_role::HOME)
        .with_is_executable(true)
        .with_has_blob(false);

    let val = serde_json::to_value(&filter).unwrap();
    assert_eq!(val["parentId"], "dir-root");
    assert_eq!(val["descendantId"], "desc-1");
    assert_eq!(val["hasParentId"], true);
    assert_eq!(val["name"], "project");
    assert_eq!(val["nodeType"], "directory");
    assert_eq!(val["role"], "home");
    assert_eq!(val["isExecutable"], true);
    assert_eq!(val["hasBlob"], false);

    let roundtripped: FileNodeQueryFilter = serde_json::from_value(val).unwrap();
    assert_eq!(roundtripped, filter);
}

#[test]
fn test_filenode_and_refplus_capability_on_session() {
    let session_json = json!({
        "username": "user@example.com",
        "apiUrl": "https://jmap.example.com/api",
        "downloadUrl": "https://jmap.example.com/download/{blobId}",
        "uploadUrl": "https://jmap.example.com/upload",
        "state": "s123",
        "capabilities": {
            CAPABILITY_FILENODE: {
                "maxFileNodeDepth": 10,
                "maxSizeFileNodeName": 255,
                "fileNodeQuerySortOptions": ["name", "size", "modified"]
            },
            CAPABILITY_REFPLUS: {
                "jsonPath": true,
                "filterCondition": true,
                "setProperty": true
            }
        },
        "accounts": {}
    });

    let session: Session = serde_json::from_value(session_json).unwrap();
    let filenode_cap = session.filenode_capability().expect("filenode capability");
    assert_eq!(filenode_cap.max_file_node_depth, Some(10));
    assert_eq!(filenode_cap.max_size_file_node_name, Some(255));
    assert_eq!(
        filenode_cap.file_node_query_sort_options,
        vec!["name", "size", "modified"]
    );

    let refplus_cap = session.refplus_capability().expect("refplus capability");
    assert_eq!(refplus_cap.json_path, Some(true));
    assert_eq!(refplus_cap.filter_condition, Some(true));
    assert_eq!(refplus_cap.set_property, Some(true));
}

#[test]
fn test_filenode_capability_builders() {
    let cap = FileNodeCapability::new()
        .with_max_file_node_depth(20)
        .with_max_size_file_node_name(500)
        .with_file_node_query_sort_options(["name", "created"]);

    let val = serde_json::to_value(&cap).unwrap();
    assert_eq!(val["maxFileNodeDepth"], 20);
    assert_eq!(val["maxSizeFileNodeName"], 500);
    assert_eq!(val["fileNodeQuerySortOptions"], json!(["name", "created"]));

    let roundtripped: FileNodeCapability = serde_json::from_value(val).unwrap();
    assert_eq!(roundtripped, cap);
}

#[test]
fn test_result_reference_with_json_path() {
    let res_ref = ResultReference::new("call-1", "FileNode/query", "$.list[0].id");

    let val = serde_json::to_value(&res_ref).unwrap();
    assert_eq!(val["resultOf"], "call-1");
    assert_eq!(val["name"], "FileNode/query");
    assert_eq!(val["path"], "$.list[0].id");

    let roundtripped: ResultReference = serde_json::from_value(val).unwrap();
    assert_eq!(roundtripped, res_ref);
}

#[test]
fn test_filenode_constants() {
    assert_eq!(node_type::FILE, "file");
    assert_eq!(node_type::DIRECTORY, "directory");
    assert_eq!(node_type::SYMLINK, "symlink");
    assert_eq!(node_type::OTHER, "other");

    assert_eq!(node_role::ROOT, "root");
    assert_eq!(node_role::HOME, "home");
    assert_eq!(node_role::TRASH, "trash");
    assert_eq!(node_role::DOCUMENTS, "documents");
    assert_eq!(node_role::PICTURES, "pictures");
    assert_eq!(node_role::VIDEOS, "videos");
    assert_eq!(node_role::MUSIC, "music");
    assert_eq!(node_role::DOWNLOADS, "downloads");

    assert_eq!(filenode_set_error::NODE_HAS_CHILDREN, "nodeHasChildren");
    assert_eq!(filenode_set_error::ALREADY_EXISTS, "alreadyExists");
    assert_eq!(filenode_set_error::INVALID_NODE_TYPE, "invalidNodeType");
}
