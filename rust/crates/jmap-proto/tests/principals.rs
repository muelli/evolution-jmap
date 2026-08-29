// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for the RFC 9670 `Principal` query filter's convenience constructor.

#![cfg(feature = "principals")]

use jmap_proto::principals::PrincipalQueryFilter;

#[test]
fn principal_query_filter_email_sets_only_that_field() {
    let filter = PrincipalQueryFilter::email("alice@example.com");
    assert_eq!(filter.email.as_deref(), Some("alice@example.com"));
    assert_eq!(filter.name, None);
    assert_eq!(filter.text, None);
}

#[test]
fn principal_types_cover_rfc9670() {
    use jmap_proto::principals::principal_type::*;
    assert_eq!(INDIVIDUAL, "individual");
    assert_eq!(GROUP, "group");
    assert_eq!(RESOURCE, "resource");
    assert_eq!(LOCATION, "location");
    assert_eq!(OTHER, "other");
}

#[test]
fn busy_status_covers_jmap_calendars_draft() {
    use jmap_proto::principals::busy_status::*;
    assert_eq!(CONFIRMED, "confirmed");
    assert_eq!(TENTATIVE, "tentative");
    assert_eq!(UNAVAILABLE, "unavailable");
}

#[test]
fn busy_period_and_get_availability_response_deserialize_unknown_properties_cleanly() {
    use jmap_proto::principals::GetAvailabilityResponse;
    let value = serde_json::json!({
        "list": [
            {
                "utcStart": "2026-09-01T10:00:00Z",
                "utcEnd": "2026-09-01T11:00:00Z",
                "busyStatus": "confirmed",
                "transparency": "opaque"
            }
        ],
        "totalBusy": 1
    });

    let resp: GetAvailabilityResponse = serde_json::from_value(value).unwrap();
    assert_eq!(resp.list.len(), 1);
    assert_eq!(resp.list[0].busy_status, "confirmed");
}

#[test]
fn principal_secret_and_send_to_cover_rfc9670() {
    use jmap_proto::principals::Principal;
    let value = serde_json::json!({
        "id": "p_conf1",
        "name": "Room 404",
        "type": "location",
        "secret": "s3cr3t",
        "sendTo": {
            "imip": "mailto:room404@example.com"
        }
    });

    let p: Principal = serde_json::from_value(value).unwrap();
    assert_eq!(p.id.as_ref().unwrap().as_str(), "p_conf1");
    assert_eq!(p.name, "Room 404");
    assert_eq!(p.principal_type.as_deref(), Some("location"));
    assert_eq!(p.extra.get("secret"), Some(&serde_json::json!("s3cr3t")));
    assert_eq!(
        p.extra.get("sendTo"),
        Some(&serde_json::json!({"imip": "mailto:room404@example.com"}))
    );
}

#[test]
fn share_notification_roundtrip_covers_rfc9670() {
    use jmap_proto::principals::{Principal, ShareNotification, share_notification_object_type};
    use jmap_proto::state::UtcDate;
    use std::collections::BTreeMap;

    assert_eq!(share_notification_object_type::ADDRESS_BOOK, "AddressBook");
    assert_eq!(share_notification_object_type::CALENDAR, "Calendar");
    assert_eq!(share_notification_object_type::MAILBOX, "Mailbox");

    let notif = ShareNotification {
        id: Some("sn_1".into()),
        created: UtcDate::new("2026-09-01T14:00:00Z"),
        changed_by: Some(Principal {
            name: "Alice Admin".to_owned(),
            email: Some("alice@example.com".to_owned()),
            ..Principal::default()
        }),
        object_type: share_notification_object_type::CALENDAR.to_owned(),
        object_id: "cal_123".into(),
        account_id: "A1".into(),
        old_rights: Some(serde_json::json!({"mayReadItems": true})),
        new_rights: Some(serde_json::json!({"mayReadItems": true, "mayAddItems": true})),
        extra: BTreeMap::new(),
    };

    let n_val = serde_json::to_value(&notif).unwrap();
    assert_eq!(n_val["id"], "sn_1");
    assert_eq!(n_val["objectType"], "Calendar");
    assert_eq!(n_val["objectId"], "cal_123");
    assert_eq!(n_val["accountId"], "A1");
    assert_eq!(n_val["changedBy"]["name"], "Alice Admin");
    assert_eq!(n_val["newRights"]["mayAddItems"], true);

    let round_tripped: ShareNotification = serde_json::from_value(n_val).unwrap();
    assert_eq!(round_tripped, notif);
}

/// PrincipalsCapability and principal_set_error cover RFC 9670 §1.3 and §2.
#[test]
fn principals_capabilities_and_set_error_roundtrip_covers_rfc9670() {
    use jmap_proto::principals::{PrincipalsCapability, principal_set_error};
    use std::collections::BTreeMap;

    assert_eq!(principal_set_error::FORBIDDEN, "forbidden");
    assert_eq!(
        principal_set_error::PRINCIPAL_ALREADY_EXISTS,
        "principalAlreadyExists"
    );

    let cap = PrincipalsCapability {
        max_principals_per_get: Some(100),
        extra: BTreeMap::new(),
    };
    let cap_val = serde_json::to_value(&cap).unwrap();
    assert_eq!(cap_val["maxPrincipalsPerGet"], 100);

    let round_cap: PrincipalsCapability = serde_json::from_value(cap_val).unwrap();
    assert_eq!(round_cap, cap);
}
