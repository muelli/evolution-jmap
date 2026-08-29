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
    assert_eq!(p.secret.as_deref(), Some("s3cr3t"));
    assert_eq!(
        p.send_to.as_ref().unwrap()["imip"],
        "mailto:room404@example.com"
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

#[test]
fn principal_secret_send_to_typed_and_query_filter_builders_roundtrip() {
    use jmap_proto::principals::{Principal, PrincipalQueryFilter};
    use std::collections::BTreeMap;

    let p = Principal {
        id: Some("p_typed".into()),
        name: "Meeting Room 1".to_owned(),
        secret: Some("secret123".to_owned()),
        send_to: Some(BTreeMap::from([(
            "imip".to_owned(),
            "mailto:room1@example.com".to_owned(),
        )])),
        ..Principal::default()
    };

    let p_val = serde_json::to_value(&p).unwrap();
    assert_eq!(p_val["secret"], "secret123");
    assert_eq!(p_val["sendTo"]["imip"], "mailto:room1@example.com");

    let round_p: Principal = serde_json::from_value(p_val).unwrap();
    assert_eq!(round_p, p);

    let filter = PrincipalQueryFilter::email("room1@example.com")
        .name("Meeting Room")
        .text("Room 1");

    assert_eq!(filter.name.as_deref(), Some("Meeting Room"));
    assert_eq!(filter.email.as_deref(), Some("room1@example.com"));
    assert_eq!(filter.text.as_deref(), Some("Room 1"));
}

#[test]
fn principal_is_personal_spec_roundtrip() {
    use jmap_proto::principals::Principal;

    let p = Principal {
        id: Some("p_self".into()),
        name: "Self Principal".to_owned(),
        is_personal: Some(true),
        ..Principal::default()
    };
    let p_val = serde_json::to_value(&p).unwrap();
    assert_eq!(p_val["isPersonal"], true);

    let round: Principal = serde_json::from_value(p_val).unwrap();
    assert_eq!(round.is_personal, Some(true));
}

#[test]
fn share_notification_builders_roundtrip() {
    use jmap_proto::UtcDate;
    use jmap_proto::principals::{Principal, ShareNotification, share_notification_object_type};

    let changer = Principal {
        id: Some("p_admin".into()),
        name: "Admin User".to_owned(),
        ..Principal::default()
    };

    let notif = ShareNotification::new(
        UtcDate::new("2026-09-01T15:00:00Z"),
        share_notification_object_type::CALENDAR,
        "cal_team",
        "acc_alice",
    )
    .with_id("notif_1")
    .with_changed_by(changer.clone())
    .with_old_rights(serde_json::json!({"mayRead": true}))
    .with_new_rights(serde_json::json!({"mayRead": true, "mayWrite": true}));

    assert_eq!(notif.id.as_ref().unwrap().as_str(), "notif_1");
    assert_eq!(notif.created.as_str(), "2026-09-01T15:00:00Z");
    assert_eq!(notif.object_type, "Calendar");
    assert_eq!(notif.object_id.as_str(), "cal_team");
    assert_eq!(notif.account_id.as_str(), "acc_alice");
    assert_eq!(notif.changed_by.as_ref().unwrap().name, "Admin User");

    let notif_val = serde_json::to_value(&notif).unwrap();
    assert_eq!(notif_val["id"], "notif_1");
    assert_eq!(notif_val["created"], "2026-09-01T15:00:00Z");
    assert_eq!(notif_val["objectType"], "Calendar");
    assert_eq!(notif_val["objectId"], "cal_team");
    assert_eq!(notif_val["accountId"], "acc_alice");
    assert_eq!(notif_val["changedBy"]["name"], "Admin User");
    assert_eq!(notif_val["oldRights"]["mayRead"], true);
    assert_eq!(notif_val["newRights"]["mayWrite"], true);
    assert_eq!(
        serde_json::from_value::<ShareNotification>(notif_val).unwrap(),
        notif
    );
}

#[test]
fn principal_and_availability_builders() {
    use jmap_proto::UtcDate;
    use jmap_proto::calendars::CalendarEvent;
    use jmap_proto::principals::{BusyPeriod, GetAvailabilityResponse, Principal, principal_type};
    use std::collections::BTreeMap;

    let p = Principal::new("Conference Room 1")
        .with_id("p_conf1")
        .with_type(principal_type::RESOURCE)
        .with_email("room1@example.com")
        .with_description("Projector equipped")
        .with_time_zone("UTC")
        .with_secret("secret123")
        .with_send_to(BTreeMap::from([(
            "imip".to_string(),
            "mailto:room1@example.com".to_string(),
        )]))
        .with_capabilities(BTreeMap::from([(
            "urn:ietf:params:jmap:principals:owner".to_string(),
            serde_json::json!({}),
        )]))
        .is_personal(false);

    assert_eq!(p.name, "Conference Room 1");
    assert_eq!(p.id.as_ref().unwrap().as_str(), "p_conf1");
    assert_eq!(p.principal_type.as_deref(), Some("resource"));
    assert_eq!(p.email.as_deref(), Some("room1@example.com"));
    assert_eq!(p.description.as_deref(), Some("Projector equipped"));
    assert_eq!(p.time_zone.as_deref(), Some("UTC"));
    assert_eq!(p.secret.as_deref(), Some("secret123"));
    assert_eq!(p.is_personal, Some(false));
    assert!(
        p.capabilities
            .contains_key("urn:ietf:params:jmap:principals:owner")
    );

    let p_val = serde_json::to_value(&p).unwrap();
    assert_eq!(p_val["name"], "Conference Room 1");
    assert_eq!(p_val["id"], "p_conf1");
    assert_eq!(p_val["type"], "resource");
    assert_eq!(p_val["isPersonal"], false);
    assert_eq!(serde_json::from_value::<Principal>(p_val).unwrap(), p);

    let avail = GetAvailabilityResponse::new([BusyPeriod::new(
        UtcDate::new("2026-09-01T10:00:00Z"),
        UtcDate::new("2026-09-01T11:00:00Z"),
        "busy",
    )
    .with_event(CalendarEvent::default())]);

    assert_eq!(avail.list.len(), 1);
    assert_eq!(avail.list[0].busy_status, "busy");
    assert!(avail.list[0].event.is_some());
    let avail_val = serde_json::to_value(&avail).unwrap();
    assert_eq!(avail_val["list"][0]["busyStatus"], "busy");
    assert_eq!(
        serde_json::from_value::<GetAvailabilityResponse>(avail_val).unwrap(),
        avail
    );
}

#[test]
fn principals_capability_builder() {
    use jmap_proto::principals::PrincipalsCapability;

    let cap = PrincipalsCapability::new().with_max_principals_per_get(250);
    assert_eq!(cap.max_principals_per_get, Some(250));
    let val = serde_json::to_value(&cap).unwrap();
    assert_eq!(val["maxPrincipalsPerGet"], 250);
    assert_eq!(
        serde_json::from_value::<PrincipalsCapability>(val).unwrap(),
        cap
    );
}
