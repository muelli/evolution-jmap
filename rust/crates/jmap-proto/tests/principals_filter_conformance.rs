// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::Id;
use jmap_proto::principals::{
    PrincipalQueryFilter, PrincipalsCapability, ShareNotificationQueryFilter,
};
use serde_json::json;

#[test]
fn principals_capability_supports_current_user_principal_id() {
    let json = json!({
        "currentUserPrincipalId": "p123",
        "maxPrincipalsPerGet": 50
    });
    let cap: PrincipalsCapability =
        serde_json::from_value(json).expect("deserialize PrincipalsCapability");
    assert_eq!(cap.current_user_principal_id, Some(Id::new("p123")));
    assert_eq!(cap.max_principals_per_get, Some(50));

    let constructed = PrincipalsCapability::new()
        .with_current_user_principal_id("p456")
        .with_max_principals_per_get(100);
    let val = serde_json::to_value(&constructed).expect("serialize PrincipalsCapability");
    assert_eq!(val["currentUserPrincipalId"], "p456");
    assert_eq!(val["maxPrincipalsPerGet"], 100);
}

#[test]
fn principal_query_filter_supports_rfc9670_fields() {
    let json = json!({
        "accountIds": ["a1", "a2"],
        "name": "Jane",
        "email": "jane@example.com",
        "text": "finance",
        "type": "individual",
        "timeZone": "Europe/Berlin"
    });
    let filter: PrincipalQueryFilter =
        serde_json::from_value(json).expect("deserialize PrincipalQueryFilter");
    assert_eq!(filter.account_ids, Some(vec![Id::new("a1"), Id::new("a2")]));
    assert_eq!(filter.name.as_deref(), Some("Jane"));
    assert_eq!(filter.email.as_deref(), Some("jane@example.com"));
    assert_eq!(filter.text.as_deref(), Some("finance"));
    assert_eq!(filter.principal_type.as_deref(), Some("individual"));
    assert_eq!(filter.time_zone.as_deref(), Some("Europe/Berlin"));

    let built = PrincipalQueryFilter::default()
        .account_ids(vec!["acc1", "acc2"])
        .principal_type("group")
        .time_zone("UTC");
    let val = serde_json::to_value(&built).expect("serialize PrincipalQueryFilter");
    assert_eq!(val["accountIds"], json!(["acc1", "acc2"]));
    assert_eq!(val["type"], "group");
    assert_eq!(val["timeZone"], "UTC");
}

#[test]
fn share_notification_query_filter_supports_object_account_id() {
    let json = json!({
        "objectAccountId": "acc123",
        "objectType": "Calendar"
    });
    let filter: ShareNotificationQueryFilter =
        serde_json::from_value(json).expect("deserialize ShareNotificationQueryFilter");
    assert_eq!(filter.object_account_id, Some(Id::new("acc123")));
    assert_eq!(filter.object_type.as_deref(), Some("Calendar"));

    let built = ShareNotificationQueryFilter::new().with_object_account_id("acc999");
    let val = serde_json::to_value(&built).expect("serialize ShareNotificationQueryFilter");
    assert_eq!(val["objectAccountId"], "acc999");
}
