// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::metadata::{
    DataTypeMetadataInfo, MetadataCapability, MetadataFilterCondition, MetadataTextFilter,
};
use jmap_proto::session::{CAPABILITY_METADATA, Session};
use serde_json::json;

#[test]
fn metadata_capability_roundtrip_and_builders() {
    let info = DataTypeMetadataInfo::new()
        .with_namespaces(vec![
            "urn:ietf:params:jmap:metadata:notes",
            "custom.vendor.ns",
        ])
        .supports_vendor_namespaces(true)
        .supports_private(true)
        .with_max_depth(5);

    assert_eq!(info.namespaces.len(), 2);
    assert!(info.supports_vendor_namespaces);
    assert!(info.supports_private);
    assert_eq!(info.max_depth, Some(5));

    let cap = MetadataCapability::new().with_data_type("Email", info);
    assert!(cap.data_types.contains_key("Email"));

    let json_val = serde_json::to_value(&cap).expect("serializes MetadataCapability");
    assert_eq!(
        json_val["dataTypes"]["Email"]["namespaces"][0],
        "urn:ietf:params:jmap:metadata:notes"
    );
    assert_eq!(
        json_val["dataTypes"]["Email"]["supportsVendorNamespaces"],
        true
    );
    assert_eq!(json_val["dataTypes"]["Email"]["supportsPrivate"], true);
    assert_eq!(json_val["dataTypes"]["Email"]["maxDepth"], 5);

    let roundtripped: MetadataCapability =
        serde_json::from_value(json_val).expect("deserializes MetadataCapability");
    assert_eq!(roundtripped, cap);
}

#[test]
fn metadata_filter_condition_builders_and_roundtrip() {
    let filter = MetadataFilterCondition::new()
        .with_metadata_exists("vendor.tags.priority")
        .with_metadata_text_contains("vendor.tags.status", "urgent")
        .with_private_metadata_text_equals("user.category", "personal");

    let json_val = serde_json::to_value(&filter).expect("serializes MetadataFilterCondition");
    assert_eq!(json_val["metadataExists"], "vendor.tags.priority");
    assert_eq!(
        json_val["metadataTextContains"]["path"],
        "vendor.tags.status"
    );
    assert_eq!(json_val["metadataTextContains"]["text"], "urgent");
    assert_eq!(
        json_val["privateMetadataTextEquals"]["path"],
        "user.category"
    );
    assert_eq!(json_val["privateMetadataTextEquals"]["text"], "personal");

    let parsed: MetadataFilterCondition =
        serde_json::from_value(json_val).expect("deserializes MetadataFilterCondition");
    assert_eq!(parsed, filter);
}

#[test]
fn metadata_text_filter_roundtrip_and_builders() {
    let text_filter = MetadataTextFilter::new("vendor.flag.category", "starred").with_extra(
        json!({"collation": "i;unicode-casemap"})
            .as_object()
            .unwrap()
            .clone()
            .into_iter()
            .collect(),
    );

    assert_eq!(text_filter.path, "vendor.flag.category");
    assert_eq!(text_filter.text, "starred");
    assert_eq!(text_filter.extra["collation"], "i;unicode-casemap");

    let val = serde_json::to_value(&text_filter).expect("serializes MetadataTextFilter");
    assert_eq!(val["path"], "vendor.flag.category");
    assert_eq!(val["text"], "starred");
    assert_eq!(val["collation"], "i;unicode-casemap");

    let roundtripped: MetadataTextFilter =
        serde_json::from_value(val).expect("deserializes MetadataTextFilter");
    assert_eq!(roundtripped, text_filter);
}

#[test]
fn session_metadata_capability_accessor() {
    let raw = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {},
            "urn:ietf:params:jmap:metadata": {
                "dataTypes": {
                    "CalendarEvent": {
                        "namespaces": ["urn:ietf:params:jmap:metadata:events"],
                        "supportsVendorNamespaces": true,
                        "supportsPrivate": false,
                        "maxDepth": 3
                    }
                }
            }
        },
        "accounts": {},
        "primaryAccounts": {},
        "username": "user@example.com",
        "apiUrl": "https://api.example.com/jmap/",
        "downloadUrl": "https://api.example.com/download/{blobId}",
        "uploadUrl": "https://api.example.com/upload/",
        "state": "s1"
    });

    let session: Session = serde_json::from_value(raw).expect("deserializes Session");
    assert_eq!(CAPABILITY_METADATA, "urn:ietf:params:jmap:metadata");
    let meta_cap = session
        .metadata_capability()
        .expect("has metadata capability");
    assert!(meta_cap.data_types.contains_key("CalendarEvent"));
    let event_info = &meta_cap.data_types["CalendarEvent"];
    assert_eq!(
        event_info.namespaces,
        vec!["urn:ietf:params:jmap:metadata:events".to_string()]
    );
    assert!(event_info.supports_vendor_namespaces);
    assert!(!event_info.supports_private);
    assert_eq!(event_info.max_depth, Some(3));
}
