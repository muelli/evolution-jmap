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
