// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9425 (JMAP for Quotas) unit and roundtrip tests.

use jmap_proto::quota::{
    Quota, QuotaCapability, QuotaQueryFilter, quota_data_type, quota_resource_type, quota_scope,
    quota_set_error,
};
use serde_json::json;

#[test]
fn quota_round_trips_through_camel_case_json() {
    let quota = Quota::new(
        "q1",
        "Storage Quota",
        quota_resource_type::OCTETS,
        512_000,
        1_000_000,
        quota_scope::ACCOUNT,
        vec![quota_data_type::MAIL, quota_data_type::CONTACTS],
    )
    .with_account_id("acc1")
    .with_warn_limit(800_000)
    .with_soft_limit(900_000)
    .with_description("Primary account storage limit");

    let val = serde_json::to_value(&quota).expect("to_value");
    assert_eq!(val["id"], "q1");
    assert_eq!(val["accountId"], "acc1");
    assert_eq!(val["name"], "Storage Quota");
    assert_eq!(val["resourceType"], "octets");
    assert_eq!(val["used"], 512_000);
    assert_eq!(val["hardLimit"], 1_000_000);
    assert_eq!(val["scope"], "account");
    assert_eq!(val["types"], json!(["Mail", "Contacts"]));
    assert_eq!(val["warnLimit"], 800_000);
    assert_eq!(val["softLimit"], 900_000);
    assert_eq!(val["description"], "Primary account storage limit");

    let round: Quota = serde_json::from_value(val).expect("from_value");
    assert_eq!(round, quota);
}

#[test]
fn quota_minimal_deserialization_and_forward_compatibility() {
    let raw = json!({
        "id": "quota_min",
        "name": "Message Count Limit",
        "resourceType": "count",
        "used": 1500,
        "hardLimit": 5000,
        "scope": "domain",
        "types": ["Mail"],
        "futureQuotaProperty": "unlimited-archive"
    });

    let q: Quota = serde_json::from_value(raw).expect("deserializes minimal quota");
    assert_eq!(q.id.as_str(), "quota_min");
    assert_eq!(q.name, "Message Count Limit");
    assert_eq!(q.resource_type, "count");
    assert_eq!(q.used, 1500);
    assert_eq!(q.hard_limit, 5000);
    assert_eq!(q.scope, "domain");
    assert_eq!(q.types, vec!["Mail".to_string()]);
    assert!(q.account_id.is_none());
    assert!(q.warn_limit.is_none());
    assert!(q.soft_limit.is_none());
    assert!(q.description.is_none());
    assert_eq!(q.extra["futureQuotaProperty"], "unlimited-archive");
}

#[test]
fn quota_query_filter_round_trips_and_builders() {
    let filter = QuotaQueryFilter::new()
        .with_name("Storage")
        .with_resource_type(quota_resource_type::OCTETS)
        .with_scope(quota_scope::ACCOUNT)
        .with_type(quota_data_type::MAIL);

    let val = serde_json::to_value(&filter).expect("to_value");
    assert_eq!(val["name"], "Storage");
    assert_eq!(val["resourceType"], "octets");
    assert_eq!(val["scope"], "account");
    assert_eq!(val["type"], "Mail");

    let round: QuotaQueryFilter = serde_json::from_value(val).expect("from_value");
    assert_eq!(round, filter);
}

#[test]
fn quota_capability_and_constants_coverage() {
    assert_eq!(quota_resource_type::OCTETS, "octets");
    assert_eq!(quota_resource_type::COUNT, "count");
    assert_eq!(quota_scope::ACCOUNT, "account");
    assert_eq!(quota_scope::DOMAIN, "domain");
    assert_eq!(quota_scope::GLOBAL, "global");
    assert_eq!(quota_data_type::MAIL, "Mail");
    assert_eq!(quota_data_type::CONTACTS, "Contacts");
    assert_eq!(quota_data_type::CALENDARS, "Calendars");
    assert_eq!(quota_set_error::OVER_QUOTA, "overQuota");

    let cap = QuotaCapability::new();
    let val = serde_json::to_value(&cap).expect("to_value");
    assert_eq!(val, json!({}));
    let round: QuotaCapability = serde_json::from_value(val).expect("from_value");
    assert_eq!(round, cap);
}
