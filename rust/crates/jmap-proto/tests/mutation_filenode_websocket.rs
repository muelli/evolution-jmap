// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::filenode::{
    FileNode, FileNodeCapability, FileNodeQueryFilter, FileNodeRights, filenode_set_error,
    node_role, node_type,
};
use jmap_proto::id::Id;
use jmap_proto::request::Invocation;
use jmap_proto::state::{State, UtcDate};
use jmap_proto::websocket::{
    SUBPROTOCOL, WebSocketPushDisable, WebSocketPushEnable, WebSocketRequest,
    WebSocketRequestError, WebSocketResponse, message_type,
};
use serde_json::json;

#[test]
fn filenode_rights_builders_and_properties() {
    let rights = FileNodeRights::default()
        .with_may_read(true)
        .with_may_write(false)
        .with_may_admin(true)
        .with_may_modify_content(false);

    assert_eq!(rights.may_read, Some(true));
    assert_eq!(rights.may_write, Some(false));
    assert_eq!(rights.may_admin, Some(true));
    assert_eq!(rights.may_modify_content, Some(false));
    assert!(!rights.is_writable());

    let val = serde_json::to_value(&rights).expect("serialize FileNodeRights");
    assert_eq!(val["mayRead"], true);
    assert_eq!(val["mayWrite"], false);
    assert_eq!(val["mayAdmin"], true);
    assert_eq!(val["mayModifyContent"], false);

    let roundtripped: FileNodeRights =
        serde_json::from_value(val).expect("deserialize FileNodeRights");
    assert_eq!(roundtripped, rights);

    let writable_rights = rights.with_may_write(true);
    assert_eq!(writable_rights.may_write, Some(true));
    assert!(writable_rights.is_writable());
}

#[test]
fn filenode_capability_with_extra_retains_custom_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorTier".to_string(), json!("ceph-distributed"));
    extra.insert("quotaEnforced".to_string(), json!(true));

    let cap = FileNodeCapability::new()
        .with_max_file_node_depth(16)
        .with_max_size_file_node_name(1024)
        .with_file_node_query_sort_options(["name", "size"])
        .with_extra(extra);

    assert_eq!(cap.max_file_node_depth, Some(16));
    assert_eq!(cap.max_size_file_node_name, Some(1024));
    assert_eq!(cap.file_node_query_sort_options, vec!["name", "size"]);
    assert_eq!(
        cap.extra.get("customVendorTier"),
        Some(&json!("ceph-distributed"))
    );
    assert_eq!(cap.extra.get("quotaEnforced"), Some(&json!(true)));

    let val = serde_json::to_value(&cap).expect("serialize FileNodeCapability");
    assert_eq!(val["maxFileNodeDepth"], 16);
    assert_eq!(val["maxSizeFileNodeName"], 1024);
    assert_eq!(val["fileNodeQuerySortOptions"], json!(["name", "size"]));
    assert_eq!(val["customVendorTier"], "ceph-distributed");
    assert_eq!(val["quotaEnforced"], true);

    let roundtripped: FileNodeCapability =
        serde_json::from_value(val).expect("deserialize FileNodeCapability");
    assert_eq!(roundtripped, cap);
}

#[test]
fn filenode_query_filter_with_extra_retains_custom_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customIndexScan".to_string(), json!("btree-index"));

    let filter = FileNodeQueryFilter::new()
        .with_parent_id("dir-root")
        .with_has_blob(true)
        .with_extra(extra);

    assert_eq!(filter.parent_id, Some(Id::from("dir-root")));
    assert_eq!(filter.has_blob, Some(true));
    assert_eq!(
        filter.extra.get("customIndexScan"),
        Some(&json!("btree-index"))
    );

    let val = serde_json::to_value(&filter).expect("serialize FileNodeQueryFilter");
    assert_eq!(val["parentId"], "dir-root");
    assert_eq!(val["hasBlob"], true);
    assert_eq!(val["customIndexScan"], "btree-index");

    let roundtripped: FileNodeQueryFilter =
        serde_json::from_value(val).expect("deserialize FileNodeQueryFilter");
    assert_eq!(roundtripped, filter);
}

#[test]
fn filenode_with_extra_retains_custom_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("storageClass".to_string(), json!("glacier-deep-archive"));
    extra.insert("encryptionKeyId".to_string(), json!("key-kms-42"));

    let node = FileNode::new("fn-456", "archive.tar.gz", node_type::FILE)
        .with_size(10485760)
        .with_extra(extra);

    assert_eq!(node.id, Id::from("fn-456"));
    assert_eq!(node.name, "archive.tar.gz");
    assert_eq!(node.node_type, "file");
    assert_eq!(node.size, Some(10485760));
    assert_eq!(
        node.extra.get("storageClass"),
        Some(&json!("glacier-deep-archive"))
    );
    assert_eq!(
        node.extra.get("encryptionKeyId"),
        Some(&json!("key-kms-42"))
    );

    let val = serde_json::to_value(&node).expect("serialize FileNode");
    assert_eq!(val["id"], "fn-456");
    assert_eq!(val["name"], "archive.tar.gz");
    assert_eq!(val["nodeType"], "file");
    assert_eq!(val["size"], 10485760);
    assert_eq!(val["storageClass"], "glacier-deep-archive");
    assert_eq!(val["encryptionKeyId"], "key-kms-42");

    let roundtripped: FileNode = serde_json::from_value(val).expect("deserialize FileNode");
    assert_eq!(roundtripped, node);
}

#[test]
fn filenode_all_builders_and_standard_constants() {
    let now = UtcDate::new("2026-10-16T12:00:00Z");
    let modified = UtcDate::new("2026-10-16T12:05:00Z");
    let rights = FileNodeRights::all();

    let node = FileNode::new("fn-99", "build.sh", node_type::FILE)
        .with_parent_id("dir-bin")
        .with_blob_id("blob-bin-1")
        .with_size(512)
        .with_node_role(node_role::DOCUMENTS)
        .with_created(now.clone())
        .with_modified(modified.clone())
        .with_executable(true)
        .with_my_rights(rights.clone())
        .with_sha256("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789");

    assert_eq!(node.id, Id::from("fn-99"));
    assert_eq!(node.parent_id, Some(Id::from("dir-bin")));
    assert_eq!(node.name, "build.sh");
    assert_eq!(node.blob_id, Some(Id::from("blob-bin-1")));
    assert_eq!(node.size, Some(512));
    assert_eq!(node.node_type, "file");
    assert_eq!(node.node_role.as_deref(), Some("documents"));
    assert_eq!(node.created, Some(now));
    assert_eq!(node.modified, Some(modified));
    assert_eq!(node.executable, Some(true));
    assert_eq!(node.my_rights, Some(rights));
    assert_eq!(
        node.sha256.as_deref(),
        Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
    );

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

#[test]
fn websocket_request_builder_invocation_and_created_ids() {
    let inv1 =
        Invocation::new("Mailbox/get", &json!({"accountId": "acc1"}), "c1").expect("invocation c1");
    let inv2 =
        Invocation::new("Email/query", &json!({"accountId": "acc1"}), "c2").expect("invocation c2");

    let mut created_ids = BTreeMap::new();
    created_ids.insert(Id::from("temp-1"), Id::from("real-1"));

    let req = WebSocketRequest::new(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
        .with_id("ws-req-101")
        .with_invocation(inv1.clone())
        .with_invocation(inv2.clone())
        .with_created_ids(created_ids.clone());

    assert_eq!(req.message_type, message_type::REQUEST);
    assert_eq!(req.id.as_deref(), Some("ws-req-101"));
    assert_eq!(
        req.using,
        vec![
            "urn:ietf:params:jmap:core".to_string(),
            "urn:ietf:params:jmap:mail".to_string(),
        ]
    );
    assert_eq!(req.method_calls.len(), 2);
    assert_eq!(req.method_calls[0], inv1);
    assert_eq!(req.method_calls[1], inv2);
    assert_eq!(req.created_ids, Some(created_ids));

    let val = serde_json::to_value(&req).expect("serialize WebSocketRequest");
    assert_eq!(val["@type"], "Request");
    assert_eq!(val["id"], "ws-req-101");
    assert_eq!(val["createdIds"]["temp-1"], "real-1");

    let roundtripped: WebSocketRequest =
        serde_json::from_value(val).expect("deserialize WebSocketRequest");
    assert_eq!(roundtripped, req);
}

#[test]
fn websocket_response_builders_and_filtering() {
    let inv_c1_1 = Invocation::new("Mailbox/get", &json!({"state": "s1"}), "c1").unwrap();
    let inv_c1_2 = Invocation::new("Mailbox/changes", &json!({"hasMore": false}), "c1").unwrap();
    let inv_c2 = Invocation::new("Email/query", &json!({"ids": ["e1", "e2"]}), "c2").unwrap();

    let mut created_ids = BTreeMap::new();
    created_ids.insert(Id::from("temp-2"), Id::from("real-2"));

    let resp = WebSocketResponse::new()
        .with_id("ws-resp-202")
        .with_session_state(State::from("session-state-token"))
        .with_method_response(inv_c1_1.clone())
        .with_method_response(inv_c2.clone())
        .with_method_response(inv_c1_2.clone())
        .with_created_ids(created_ids.clone());

    assert_eq!(resp.message_type, message_type::RESPONSE);
    assert_eq!(resp.id.as_deref(), Some("ws-resp-202"));
    assert_eq!(resp.session_state, Some(State::from("session-state-token")));
    assert_eq!(resp.created_ids, Some(created_ids));

    let c1_responses: Vec<_> = resp.responses_for("c1").cloned().collect();
    assert_eq!(c1_responses.len(), 2);
    assert_eq!(c1_responses[0], inv_c1_1);
    assert_eq!(c1_responses[1], inv_c1_2);

    let c2_responses: Vec<_> = resp.responses_for("c2").cloned().collect();
    assert_eq!(c2_responses.len(), 1);
    assert_eq!(c2_responses[0], inv_c2);

    let c3_responses: Vec<_> = resp.responses_for("nonexistent").collect();
    assert!(c3_responses.is_empty());

    let val = serde_json::to_value(&resp).expect("serialize WebSocketResponse");
    assert_eq!(val["@type"], "Response");
    assert_eq!(val["id"], "ws-resp-202");
    assert_eq!(val["sessionState"], "session-state-token");
    assert_eq!(val["createdIds"]["temp-2"], "real-2");

    let roundtripped: WebSocketResponse =
        serde_json::from_value(val).expect("deserialize WebSocketResponse");
    assert_eq!(roundtripped, resp);
}

#[test]
fn websocket_request_error_builders_and_properties() {
    let err = WebSocketRequestError::new("urn:ietf:params:jmap:error:unknownCapability")
        .with_id("ws-err-303")
        .with_status(400)
        .with_detail("Capability urn:example:unknown is not supported");

    assert_eq!(err.message_type, message_type::REQUEST_ERROR);
    assert_eq!(err.id.as_deref(), Some("ws-err-303"));
    assert_eq!(
        err.error_type,
        "urn:ietf:params:jmap:error:unknownCapability"
    );
    assert_eq!(err.status, Some(400));
    assert_eq!(
        err.detail.as_deref(),
        Some("Capability urn:example:unknown is not supported")
    );

    let val = serde_json::to_value(&err).expect("serialize WebSocketRequestError");
    assert_eq!(val["@type"], "RequestError");
    assert_eq!(val["id"], "ws-err-303");
    assert_eq!(val["type"], "urn:ietf:params:jmap:error:unknownCapability");
    assert_eq!(val["status"], 400);
    assert_eq!(
        val["detail"],
        "Capability urn:example:unknown is not supported"
    );

    let roundtripped: WebSocketRequestError =
        serde_json::from_value(val).expect("deserialize WebSocketRequestError");
    assert_eq!(roundtripped, err);
}

#[test]
fn websocket_push_enable_and_disable_builders_and_subprotocol() {
    assert_eq!(SUBPROTOCOL, "jmap");

    let enable = WebSocketPushEnable::new().with_data_types(["Email", "Mailbox", "ContactCard"]);
    assert_eq!(enable.message_type, message_type::PUSH_ENABLE);
    assert_eq!(
        enable.data_types,
        Some(vec![
            "Email".to_string(),
            "Mailbox".to_string(),
            "ContactCard".to_string(),
        ])
    );

    let val_enable = serde_json::to_value(&enable).expect("serialize WebSocketPushEnable");
    assert_eq!(val_enable["@type"], "WebSocketPushEnable");
    assert_eq!(val_enable["dataTypes"][0], "Email");
    assert_eq!(val_enable["dataTypes"][1], "Mailbox");
    assert_eq!(val_enable["dataTypes"][2], "ContactCard");

    let roundtripped_enable: WebSocketPushEnable =
        serde_json::from_value(val_enable).expect("deserialize WebSocketPushEnable");
    assert_eq!(roundtripped_enable, enable);

    let disable = WebSocketPushDisable::new();
    assert_eq!(disable.message_type, message_type::PUSH_DISABLE);

    let val_disable = serde_json::to_value(&disable).expect("serialize WebSocketPushDisable");
    assert_eq!(val_disable["@type"], "WebSocketPushDisable");

    let roundtripped_disable: WebSocketPushDisable =
        serde_json::from_value(val_disable).expect("deserialize WebSocketPushDisable");
    assert_eq!(roundtripped_disable, disable);
}
