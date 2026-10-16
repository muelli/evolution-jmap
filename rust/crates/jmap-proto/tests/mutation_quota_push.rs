// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::id::Id;
use jmap_proto::push::{
    PushSubscription, PushSubscriptionKeys, PushVerification, StateChange, WebPushVapidCapability,
    push_subscription_set_error,
};
use jmap_proto::quota::{
    Quota, QuotaCapability, QuotaQueryFilter, quota_data_type, quota_resource_type, quota_scope,
    quota_set_error,
};
use jmap_proto::state::{State, UtcDate};
use serde_json::json;

#[test]
fn quota_with_extra_retains_custom_properties_and_attributes() {
    let mut extra = BTreeMap::new();
    extra.insert("vendorCustomAlarm".to_string(), json!(true));
    extra.insert(
        "notificationUri".to_string(),
        json!("https://notify.example.com"),
    );

    let quota = Quota::new(
        "quota_user_1",
        "Storage Quota",
        quota_resource_type::OCTETS,
        750_000,
        1_000_000,
        quota_scope::ACCOUNT,
        [quota_data_type::MAIL, quota_data_type::CONTACTS],
    )
    .with_account_id("acc_99")
    .with_warn_limit(800_000)
    .with_soft_limit(900_000)
    .with_description("Account mail and contacts storage limit")
    .with_extra(extra);

    assert_eq!(quota.id, Id::from("quota_user_1"));
    assert_eq!(quota.account_id, Some(Id::from("acc_99")));
    assert_eq!(quota.name, "Storage Quota");
    assert_eq!(quota.resource_type, "octets");
    assert_eq!(quota.used, 750_000);
    assert_eq!(quota.hard_limit, 1_000_000);
    assert_eq!(quota.scope, "account");
    assert_eq!(
        quota.types,
        vec!["Email".to_string(), "ContactCard".to_string()]
    );
    assert_eq!(quota.warn_limit, Some(800_000));
    assert_eq!(quota.soft_limit, Some(900_000));
    assert_eq!(
        quota.description.as_deref(),
        Some("Account mail and contacts storage limit")
    );
    assert_eq!(quota.extra["vendorCustomAlarm"], json!(true));
    assert_eq!(
        quota.extra["notificationUri"],
        json!("https://notify.example.com")
    );

    let val = serde_json::to_value(&quota).expect("serializes Quota with extra");
    assert_eq!(val["id"], "quota_user_1");
    assert_eq!(val["accountId"], "acc_99");
    assert_eq!(val["name"], "Storage Quota");
    assert_eq!(val["resourceType"], "octets");
    assert_eq!(val["used"], 750_000);
    assert_eq!(val["hardLimit"], 1_000_000);
    assert_eq!(val["scope"], "account");
    assert_eq!(val["types"], json!(["Email", "ContactCard"]));
    assert_eq!(val["warnLimit"], 800_000);
    assert_eq!(val["softLimit"], 900_000);
    assert_eq!(
        val["description"],
        "Account mail and contacts storage limit"
    );
    assert_eq!(val["vendorCustomAlarm"], true);
    assert_eq!(val["notificationUri"], "https://notify.example.com");

    let round: Quota = serde_json::from_value(val).expect("deserializes Quota with extra");
    assert_eq!(round, quota);
}

#[test]
fn quota_query_filter_with_extra_retains_extension_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("filterVersion".to_string(), json!(2));
    extra.insert("caseSensitive".to_string(), json!(false));

    let filter = QuotaQueryFilter::new()
        .with_name("Main Mailbox")
        .with_resource_type(quota_resource_type::OCTETS)
        .with_scope(quota_scope::ACCOUNT)
        .with_type(quota_data_type::MAIL)
        .with_extra(extra);

    assert_eq!(filter.name.as_deref(), Some("Main Mailbox"));
    assert_eq!(filter.resource_type.as_deref(), Some("octets"));
    assert_eq!(filter.scope.as_deref(), Some("account"));
    assert_eq!(filter.r#type.as_deref(), Some("Email"));
    assert_eq!(filter.extra["filterVersion"], json!(2));
    assert_eq!(filter.extra["caseSensitive"], json!(false));

    let val = serde_json::to_value(&filter).expect("serializes QuotaQueryFilter with extra");
    assert_eq!(val["name"], "Main Mailbox");
    assert_eq!(val["resourceType"], "octets");
    assert_eq!(val["scope"], "account");
    assert_eq!(val["type"], "Email");
    assert_eq!(val["filterVersion"], 2);
    assert_eq!(val["caseSensitive"], false);

    let round: QuotaQueryFilter =
        serde_json::from_value(val).expect("deserializes QuotaQueryFilter with extra");
    assert_eq!(round, filter);
}

#[test]
fn push_subscription_with_verification_code_and_extra() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorHeader".to_string(), json!("val123"));

    let sub = PushSubscription::new("client_dev_42", "https://push.example.com/sub/42")
        .with_id("push_sub_1")
        .with_device_client_id("client_dev_updated")
        .with_url("https://push.example.com/sub/v2")
        .with_keys(
            "BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4Y12g",
            "5IAReA-NaqDDBt6S7vdsmw",
        )
        .with_expires(UtcDate::from("2026-12-31T23:59:59Z"))
        .with_types(["Mailbox", "Email", "CalendarEvent"])
        .with_verification_code("verify_code_xyz789")
        .with_extra(extra);

    assert_eq!(sub.id, Some(Id::from("push_sub_1")));
    assert_eq!(sub.device_client_id, "client_dev_updated");
    assert_eq!(sub.url.as_deref(), Some("https://push.example.com/sub/v2"));
    let keys = sub.keys.as_ref().expect("keys should be set");
    assert_eq!(keys.p256dh, "BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4Y12g");
    assert_eq!(keys.auth, "5IAReA-NaqDDBt6S7vdsmw");
    assert_eq!(
        sub.expires,
        Some(UtcDate::from("2026-12-31T23:59:59Z".to_string()))
    );
    assert_eq!(
        sub.types,
        Some(vec![
            "Mailbox".to_string(),
            "Email".to_string(),
            "CalendarEvent".to_string(),
        ])
    );
    assert_eq!(sub.verification_code.as_deref(), Some("verify_code_xyz789"));
    assert_eq!(sub.extra["customVendorHeader"], json!("val123"));

    let val = serde_json::to_value(&sub).expect("serializes PushSubscription with extra");
    assert_eq!(val["id"], "push_sub_1");
    assert_eq!(val["deviceClientId"], "client_dev_updated");
    assert_eq!(val["url"], "https://push.example.com/sub/v2");
    assert_eq!(
        val["keys"]["p256dh"],
        "BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4Y12g"
    );
    assert_eq!(val["keys"]["auth"], "5IAReA-NaqDDBt6S7vdsmw");
    assert_eq!(val["expires"], "2026-12-31T23:59:59Z");
    assert_eq!(val["types"], json!(["Mailbox", "Email", "CalendarEvent"]));
    assert_eq!(val["verificationCode"], "verify_code_xyz789");
    assert_eq!(val["customVendorHeader"], "val123");

    let round: PushSubscription =
        serde_json::from_value(val).expect("deserializes PushSubscription with extra");
    assert_eq!(round, sub);
}

#[test]
fn push_subscription_keys_builder_methods_and_properties() {
    let keys = PushSubscriptionKeys::new("p256dh_initial", "auth_initial")
        .with_p256dh("p256dh_updated_key")
        .with_auth("auth_updated_secret");

    assert_eq!(keys.p256dh, "p256dh_updated_key");
    assert_eq!(keys.auth, "auth_updated_secret");

    let val = serde_json::to_value(&keys).expect("serializes PushSubscriptionKeys");
    assert_eq!(val["p256dh"], "p256dh_updated_key");
    assert_eq!(val["auth"], "auth_updated_secret");

    let round: PushSubscriptionKeys =
        serde_json::from_value(val).expect("deserializes PushSubscriptionKeys");
    assert_eq!(round, keys);
}

#[test]
fn web_push_vapid_capability_builders_and_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("vapidVersion".to_string(), json!("draft-02"));

    let cap = WebPushVapidCapability::new("app_server_key_init")
        .with_application_server_key("app_server_key_updated")
        .with_extra(extra);

    assert_eq!(cap.application_server_key, "app_server_key_updated");
    assert_eq!(cap.extra["vapidVersion"], json!("draft-02"));

    let val = serde_json::to_value(&cap).expect("serializes WebPushVapidCapability");
    assert_eq!(val["applicationServerKey"], "app_server_key_updated");
    assert_eq!(val["vapidVersion"], "draft-02");

    let round: WebPushVapidCapability =
        serde_json::from_value(val).expect("deserializes WebPushVapidCapability");
    assert_eq!(round, cap);
}

#[test]
fn push_verification_constructors_and_builders() {
    let mut extra = BTreeMap::new();
    extra.insert("challengeExpiry".to_string(), json!(300));

    let pv = PushVerification::new("sub_init", "code_init")
        .with_push_subscription_id("sub_final")
        .with_verification_code("code_final")
        .with_extra(extra);

    assert_eq!(pv.object_type, "PushVerification");
    assert_eq!(pv.push_subscription_id, Id::from("sub_final"));
    assert_eq!(pv.verification_code, "code_final");
    assert_eq!(pv.extra["challengeExpiry"], json!(300));

    let val = serde_json::to_value(&pv).expect("serializes PushVerification");
    assert_eq!(val["@type"], "PushVerification");
    assert_eq!(val["pushSubscriptionId"], "sub_final");
    assert_eq!(val["verificationCode"], "code_final");
    assert_eq!(val["challengeExpiry"], 300);

    let round: PushVerification =
        serde_json::from_value(val).expect("deserializes PushVerification");
    assert_eq!(round, pv);
}

#[test]
fn state_change_constructors_builders_and_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("deliveryId".to_string(), json!("evt_777"));

    let change = StateChange::new(BTreeMap::new())
        .with_account_change("acc1", "Email", "s100")
        .with_account_change("acc1", "Mailbox", "s101")
        .with_account_change("acc2", "CalendarEvent", "s200")
        .with_extra(extra);

    assert_eq!(change.kind, StateChange::TYPE);
    assert_eq!(change.kind, "StateChange");

    let acc1 = &change.changed[&Id::from("acc1")];
    assert_eq!(acc1["Email"], State::from("s100"));
    assert_eq!(acc1["Mailbox"], State::from("s101"));

    let acc2 = &change.changed[&Id::from("acc2")];
    assert_eq!(acc2["CalendarEvent"], State::from("s200"));
    assert_eq!(change.extra["deliveryId"], json!("evt_777"));

    let mut new_changed = BTreeMap::new();
    let mut types = BTreeMap::new();
    types.insert("ContactCard".to_string(), State::from("c1"));
    new_changed.insert(Id::from("acc3"), types);

    let replaced = change.with_changed(new_changed);
    assert!(replaced.changed.contains_key(&Id::from("acc3")));
    assert_eq!(
        replaced.changed[&Id::from("acc3")]["ContactCard"],
        State::from("c1")
    );

    let val = serde_json::to_value(&replaced).expect("serializes StateChange");
    assert_eq!(val["@type"], "StateChange");
    assert_eq!(val["changed"]["acc3"]["ContactCard"], "c1");
    assert_eq!(val["deliveryId"], "evt_777");

    let round: StateChange = serde_json::from_value(val).expect("deserializes StateChange");
    assert_eq!(round, replaced);
}

#[test]
fn push_and_quota_constants() {
    assert_eq!(push_subscription_set_error::INVALID_URL, "invalidUrl");
    assert_eq!(
        push_subscription_set_error::EXPIRES_TOO_FAR,
        "expiresTooFar"
    );

    assert_eq!(quota_resource_type::OCTETS, "octets");
    assert_eq!(quota_resource_type::COUNT, "count");
    assert_eq!(quota_scope::ACCOUNT, "account");
    assert_eq!(quota_scope::DOMAIN, "domain");
    assert_eq!(quota_scope::GLOBAL, "global");
    assert_eq!(quota_data_type::MAIL, "Email");
    assert_eq!(quota_data_type::CONTACTS, "ContactCard");
    assert_eq!(quota_data_type::CALENDARS, "CalendarEvent");
    assert_eq!(quota_set_error::OVER_QUOTA, "overQuota");

    let mut cap_extra = BTreeMap::new();
    cap_extra.insert("customQuotaEngine".to_string(), json!("fast"));
    let cap = QuotaCapability::new().with_extra(cap_extra);
    assert_eq!(cap.extra["customQuotaEngine"], json!("fast"));

    let cap_val = serde_json::to_value(&cap).expect("serializes QuotaCapability");
    assert_eq!(cap_val["customQuotaEngine"], "fast");
    let round_cap: QuotaCapability =
        serde_json::from_value(cap_val).expect("deserializes QuotaCapability");
    assert_eq!(round_cap, cap);
}
