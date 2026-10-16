// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::error::{MethodError, RequestError, SetError, method, request, set};
use jmap_proto::metadata::{
    DataTypeMetadataInfo, MetadataCapability, MetadataFilterCondition, MetadataTextFilter,
};
use serde_json::json;

#[test]
fn metadata_capability_with_extra_retains_custom_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorExtension".to_string(), json!("enabled"));
    extra.insert("quotaLimit".to_string(), json!(1024));

    let info = DataTypeMetadataInfo::new()
        .with_namespace("urn:ietf:params:jmap:metadata:notes")
        .supports_vendor_namespaces(true);

    let cap = MetadataCapability::new()
        .with_data_type("Email", info)
        .with_extra(extra);

    assert_eq!(cap.extra["customVendorExtension"], json!("enabled"));
    assert_eq!(cap.extra["quotaLimit"], json!(1024));
    assert!(cap.data_types.contains_key("Email"));

    let val = serde_json::to_value(&cap).expect("serializes MetadataCapability with extra");
    assert_eq!(val["customVendorExtension"], "enabled");
    assert_eq!(val["quotaLimit"], 1024);
    assert_eq!(
        val["dataTypes"]["Email"]["namespaces"][0],
        "urn:ietf:params:jmap:metadata:notes"
    );

    let round: MetadataCapability =
        serde_json::from_value(val).expect("deserializes MetadataCapability with extra");
    assert_eq!(round.extra["customVendorExtension"], json!("enabled"));
    assert_eq!(round.extra["quotaLimit"], json!(1024));
    assert_eq!(round, cap);
}

#[test]
fn data_type_metadata_info_with_namespace_and_extra() {
    let mut extra = BTreeMap::new();
    extra.insert("vendorAuditTrail".to_string(), json!(true));

    let info = DataTypeMetadataInfo::new()
        .with_namespace("urn:ietf:params:jmap:metadata:custom1")
        .with_namespace("urn:ietf:params:jmap:metadata:custom2")
        .supports_private(true)
        .supports_vendor_namespaces(false)
        .with_max_depth(8)
        .with_extra(extra);

    assert_eq!(
        info.namespaces,
        vec![
            "urn:ietf:params:jmap:metadata:custom1".to_string(),
            "urn:ietf:params:jmap:metadata:custom2".to_string(),
        ]
    );
    assert!(info.supports_private);
    assert!(!info.supports_vendor_namespaces);
    assert_eq!(info.max_depth, Some(8));
    assert_eq!(info.extra["vendorAuditTrail"], json!(true));

    let val = serde_json::to_value(&info).expect("serializes DataTypeMetadataInfo");
    assert_eq!(
        val["namespaces"][0],
        "urn:ietf:params:jmap:metadata:custom1"
    );
    assert_eq!(
        val["namespaces"][1],
        "urn:ietf:params:jmap:metadata:custom2"
    );
    assert_eq!(val["supportsPrivate"], true);
    assert_eq!(val["supportsVendorNamespaces"], false);
    assert_eq!(val["maxDepth"], 8);
    assert_eq!(val["vendorAuditTrail"], true);

    let round: DataTypeMetadataInfo =
        serde_json::from_value(val).expect("deserializes DataTypeMetadataInfo");
    assert_eq!(round, info);
}

#[test]
fn metadata_filter_condition_text_equals_private_and_extra_builders() {
    let mut extra = BTreeMap::new();
    extra.insert("searchCaseSensitive".to_string(), json!(false));

    let filter = MetadataFilterCondition::new()
        .with_metadata_text_equals("vendor.tags.status", "published")
        .with_private_metadata_exists("user.personal_notes")
        .with_private_metadata_text_contains("user.personal_notes", "action_item")
        .with_extra(extra);

    let equals = filter
        .metadata_text_equals
        .as_ref()
        .expect("metadata_text_equals should be populated");
    assert_eq!(equals.path, "vendor.tags.status");
    assert_eq!(equals.text, "published");

    assert_eq!(
        filter.private_metadata_exists.as_deref(),
        Some("user.personal_notes")
    );

    let private_contains = filter
        .private_metadata_text_contains
        .as_ref()
        .expect("private_metadata_text_contains should be populated");
    assert_eq!(private_contains.path, "user.personal_notes");
    assert_eq!(private_contains.text, "action_item");

    assert_eq!(filter.extra["searchCaseSensitive"], json!(false));

    let val = serde_json::to_value(&filter).expect("serializes MetadataFilterCondition");
    assert_eq!(val["metadataTextEquals"]["path"], "vendor.tags.status");
    assert_eq!(val["metadataTextEquals"]["text"], "published");
    assert_eq!(val["privateMetadataExists"], "user.personal_notes");
    assert_eq!(
        val["privateMetadataTextContains"]["path"],
        "user.personal_notes"
    );
    assert_eq!(val["privateMetadataTextContains"]["text"], "action_item");
    assert_eq!(val["searchCaseSensitive"], false);

    let round: MetadataFilterCondition =
        serde_json::from_value(val).expect("deserializes MetadataFilterCondition");
    assert_eq!(round, filter);
}

#[test]
fn metadata_text_filter_equality_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("locale".to_string(), json!("en-US"));

    let filter = MetadataTextFilter::new("vendor.category", "work").with_extra(extra);
    assert_eq!(filter.path, "vendor.category");
    assert_eq!(filter.text, "work");
    assert_eq!(filter.extra["locale"], json!("en-US"));

    let val = serde_json::to_value(&filter).expect("serializes MetadataTextFilter");
    assert_eq!(val["path"], "vendor.category");
    assert_eq!(val["text"], "work");
    assert_eq!(val["locale"], "en-US");

    let round: MetadataTextFilter =
        serde_json::from_value(val).expect("deserializes MetadataTextFilter");
    assert_eq!(round, filter);
}

#[test]
fn request_error_constructors_builders_and_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("limit".to_string(), json!(50_000_000));

    let mut req_err = RequestError::new(request::LIMIT)
        .with_status(413)
        .with_detail("Request payload exceeds maximum allowed size");
    req_err.extra = extra;

    assert_eq!(req_err.error_type, request::LIMIT);
    assert_eq!(req_err.status, Some(413));
    assert_eq!(
        req_err.detail.as_deref(),
        Some("Request payload exceeds maximum allowed size")
    );
    assert_eq!(req_err.extra["limit"], json!(50_000_000));

    let val = serde_json::to_value(&req_err).expect("serializes RequestError");
    assert_eq!(val["type"], request::LIMIT);
    assert_eq!(val["status"], 413);
    assert_eq!(
        val["detail"],
        "Request payload exceeds maximum allowed size"
    );
    assert_eq!(val["limit"], 50_000_000);

    let round: RequestError = serde_json::from_value(val).expect("deserializes RequestError");
    assert_eq!(round, req_err);
}

#[test]
fn method_error_constructors_builders_and_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("sinceState".to_string(), json!("s0"));

    let mut method_err = MethodError::new(method::CANNOT_CALCULATE_CHANGES)
        .with_description("The requested sinceState is too old to calculate changes");
    method_err.extra = extra;

    assert_eq!(method_err.error_type, method::CANNOT_CALCULATE_CHANGES);
    assert_eq!(
        method_err.description.as_deref(),
        Some("The requested sinceState is too old to calculate changes")
    );
    assert_eq!(method_err.extra["sinceState"], json!("s0"));

    let val = serde_json::to_value(&method_err).expect("serializes MethodError");
    assert_eq!(val["type"], method::CANNOT_CALCULATE_CHANGES);
    assert_eq!(
        val["description"],
        "The requested sinceState is too old to calculate changes"
    );
    assert_eq!(val["sinceState"], "s0");

    let round: MethodError = serde_json::from_value(val).expect("deserializes MethodError");
    assert_eq!(round, method_err);
}

#[test]
fn set_error_constructors_builders_and_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("maxAllowed".to_string(), json!(10));

    let mut set_err = SetError::new(set::INVALID_PROPERTIES)
        .with_description("Invalid property combination for object creation")
        .with_properties(["name", "email", "role"]);
    set_err.extra = extra;

    assert_eq!(set_err.error_type, set::INVALID_PROPERTIES);
    assert_eq!(
        set_err.description.as_deref(),
        Some("Invalid property combination for object creation")
    );
    assert_eq!(
        set_err.properties.as_deref(),
        Some(&["name".to_string(), "email".to_string(), "role".to_string()][..])
    );
    assert_eq!(set_err.extra["maxAllowed"], json!(10));

    let val = serde_json::to_value(&set_err).expect("serializes SetError");
    assert_eq!(val["type"], set::INVALID_PROPERTIES);
    assert_eq!(
        val["description"],
        "Invalid property combination for object creation"
    );
    assert_eq!(val["properties"], json!(["name", "email", "role"]));
    assert_eq!(val["maxAllowed"], 10);

    let round: SetError = serde_json::from_value(val).expect("deserializes SetError");
    assert_eq!(round, set_err);
}

#[test]
fn error_constants_match_rfc_specifications() {
    assert_eq!(method::UNKNOWN_METHOD, "unknownMethod");
    assert_eq!(method::INVALID_ARGUMENTS, "invalidArguments");
    assert_eq!(method::INVALID_RESULT_REFERENCE, "invalidResultReference");
    assert_eq!(method::FORBIDDEN, "forbidden");
    assert_eq!(method::ACCOUNT_NOT_FOUND, "accountNotFound");
    assert_eq!(
        method::ACCOUNT_NOT_SUPPORTED_BY_METHOD,
        "accountNotSupportedByMethod"
    );
    assert_eq!(method::ACCOUNT_READ_ONLY, "accountReadOnly");
    assert_eq!(method::SERVER_FAIL, "serverFail");
    assert_eq!(method::SERVER_UNAVAILABLE, "serverUnavailable");
    assert_eq!(method::SERVER_PARTIAL_FAIL, "serverPartialFail");
    assert_eq!(method::UNKNOWN_CAPABILITY, "unknownCapability");
    assert_eq!(method::STATE_MISMATCH, "stateMismatch");
    assert_eq!(method::FROM_STATE_MISMATCH, "fromStateMismatch");
    assert_eq!(method::CANNOT_CALCULATE_CHANGES, "cannotCalculateChanges");
    assert_eq!(method::REQUEST_TOO_LARGE, "requestTooLarge");
    assert_eq!(method::FROM_ACCOUNT_NOT_FOUND, "fromAccountNotFound");
    assert_eq!(
        method::FROM_ACCOUNT_NOT_SUPPORTED_BY_METHOD,
        "fromAccountNotSupportedByMethod"
    );
    assert_eq!(method::UNSUPPORTED_FILTER, "unsupportedFilter");
    assert_eq!(method::UNSUPPORTED_SORT, "unsupportedSort");
    assert_eq!(method::ANCHOR_NOT_FOUND, "anchorNotFound");
    assert_eq!(method::TOO_MANY_CHANGES, "tooManyChanges");
    assert_eq!(method::UNKNOWN_DATA_TYPE, "unknownDataType");

    assert_eq!(set::FORBIDDEN, "forbidden");
    assert_eq!(set::OVER_QUOTA, "overQuota");
    assert_eq!(set::TOO_LARGE, "tooLarge");
    assert_eq!(set::RATE_LIMIT, "rateLimit");
    assert_eq!(set::NOT_FOUND, "notFound");
    assert_eq!(set::INVALID_PATCH, "invalidPatch");
    assert_eq!(set::INVALID_PROPERTIES, "invalidProperties");
    assert_eq!(set::SINGLETON, "singleton");
    assert_eq!(set::WILL_DESTROY, "willDestroy");
    assert_eq!(set::STATE_MISMATCH, "stateMismatch");
    assert_eq!(set::REQUEST_TOO_LARGE, "requestTooLarge");
    assert_eq!(set::ALREADY_EXISTS, "alreadyExists");
    assert_eq!(set::CANNOT_DESTROY_ORIGINAL, "cannotDestroyOriginal");
    assert_eq!(set::CANNOT_DESTROY_DEFAULT, "cannotDestroyDefault");

    assert_eq!(
        request::UNKNOWN_CAPABILITY,
        "urn:ietf:params:jmap:error:unknownCapability"
    );
    assert_eq!(request::NOT_JSON, "urn:ietf:params:jmap:error:notJSON");
    assert_eq!(
        request::NOT_REQUEST,
        "urn:ietf:params:jmap:error:notRequest"
    );
    assert_eq!(request::LIMIT, "urn:ietf:params:jmap:error:limit");
}
