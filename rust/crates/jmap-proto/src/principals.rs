// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Principals & Sharing (RFC 9670): the `Principal` object and its
//! `/get`/`/query` methods — the shared floor both scheduling
//! (`Principal/getAvailability`, draft-ietf-jmap-calendars) and per-source
//! sharing build on. See `docs/PRINCIPALS-DESIGN.md` for the full design.
//!
//! Only `Principal/get` and `Principal/query` are modeled here: resolving an
//! email/name to a principal id and its capability bag is all Phase 0 needs.
//! `Principal/set`, `Principal/changes`, `Principal/queryChanges`, and
//! `ShareNotification` are not — this project never edits principals, and
//! nothing yet syncs them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::calendars::CalendarEvent;
use crate::id::Id;
use crate::state::UtcDate;

/// A principal (RFC 9670 §2): a person, group, resource, or room a JMAP
/// server knows about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub principal_type: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// An alphanumeric secret string to authorize access to this principal (RFC 9670 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Valid methods for sending scheduling messages to this principal (RFC 9670 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    /// Server-set, per-*principal* capability bag — distinct from the
    /// account/server capability maps in `session.rs`. This is where the
    /// calendars draft hangs `mayGetAvailability` (draft-ietf-jmap-calendars
    /// §2.2) among other extension-specific fields. Kept as a `Value` bag,
    /// like every extension-defined property here, so one server's unknown
    /// per-principal capability can't sink the whole `Principal/get`.
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<BTreeMap<Id, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_personal: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `Principal/query` filter (RFC 9670 §2): resolve a person by name, email,
/// or free text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl PrincipalQueryFilter {
    pub fn email(email: impl Into<String>) -> Self {
        Self {
            email: Some(email.into()),
            ..Self::default()
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
}

/// `Principal/getAvailability` (draft-ietf-jmap-calendars §2.2 — spec'd in
/// the calendars draft even though the object queried is a `Principal`; see
/// `docs/PRINCIPALS-DESIGN.md` §2.3). A bespoke request/response shape, not
/// the generic `GetRequest`/`GetResponse` in `methods.rs`, because the
/// argument set is its own — mirrors how `EmailImportRequest` is bespoke in
/// `mail.rs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAvailabilityRequest {
    pub account_id: Id,
    pub id: Id,
    pub utc_start: UtcDate,
    pub utc_end: UtcDate,
    #[serde(default)]
    pub show_details: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_properties: Option<Vec<String>>,
}

impl GetAvailabilityRequest {
    pub fn new(
        account_id: impl Into<Id>,
        id: impl Into<Id>,
        utc_start: impl Into<UtcDate>,
        utc_end: impl Into<UtcDate>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            id: id.into(),
            utc_start: utc_start.into(),
            utc_end: utc_end.into(),
            show_details: false,
            event_properties: None,
        }
    }

    pub fn show_details(mut self) -> Self {
        self.show_details = true;
        self
    }
}

/// Well-known principal types (RFC 9670 §2).
pub mod principal_type {
    pub const INDIVIDUAL: &str = "individual";
    pub const GROUP: &str = "group";
    pub const RESOURCE: &str = "resource";
    pub const LOCATION: &str = "location";
    pub const OTHER: &str = "other";
}

/// Well-known busy status values (draft-ietf-jmap-calendars §2.2).
pub mod busy_status {
    pub const CONFIRMED: &str = "confirmed";
    pub const TENTATIVE: &str = "tentative";
    pub const UNAVAILABLE: &str = "unavailable";
}

/// `Principal/getAvailability` response: the merged `BusyPeriod`s in the
/// requested window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetAvailabilityResponse {
    #[serde(default)]
    pub list: Vec<BusyPeriod>,
}

/// One busy interval (draft-ietf-jmap-calendars §2.2). `busy_status` is one
/// of `confirmed`, `tentative`, `unavailable`; `event` is populated only
/// when the request asked for `showDetails` and the caller may see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusyPeriod {
    pub utc_start: UtcDate,
    pub utc_end: UtcDate,
    pub busy_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<CalendarEvent>,
}

impl BusyPeriod {
    pub fn new(
        utc_start: impl Into<UtcDate>,
        utc_end: impl Into<UtcDate>,
        busy_status: impl Into<String>,
    ) -> Self {
        Self {
            utc_start: utc_start.into(),
            utc_end: utc_end.into(),
            busy_status: busy_status.into(),
            event: None,
        }
    }

    pub fn with_event(mut self, event: CalendarEvent) -> Self {
        self.event = Some(event);
        self
    }
}

/// A notification that a share was created, updated, or removed (RFC 9670 §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareNotification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    pub created: UtcDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<Principal>,
    pub object_type: String,
    pub object_id: Id,
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_rights: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_rights: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ShareNotification {
    pub fn new(
        created: impl Into<UtcDate>,
        object_type: impl Into<String>,
        object_id: impl Into<Id>,
        account_id: impl Into<Id>,
    ) -> Self {
        Self {
            id: None,
            created: created.into(),
            changed_by: None,
            object_type: object_type.into(),
            object_id: object_id.into(),
            account_id: account_id.into(),
            old_rights: None,
            new_rights: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_changed_by(mut self, changed_by: Principal) -> Self {
        self.changed_by = Some(changed_by);
        self
    }

    pub fn with_old_rights(mut self, old_rights: Value) -> Self {
        self.old_rights = Some(old_rights);
        self
    }

    pub fn with_new_rights(mut self, new_rights: Value) -> Self {
        self.new_rights = Some(new_rights);
        self
    }
}

/// Standard RFC 9670 §4 share notification object types.
pub mod share_notification_object_type {

    pub const ADDRESS_BOOK: &str = "AddressBook";
    pub const CALENDAR: &str = "Calendar";
    pub const MAILBOX: &str = "Mailbox";
}

/// Principals capability properties (RFC 9670 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalsCapability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_principals_per_get: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The `SetError` types RFC 9670 §2 adds for `Principal/set`.
pub mod principal_set_error {
    pub const FORBIDDEN: &str = "forbidden";
    pub const PRINCIPAL_ALREADY_EXISTS: &str = "principalAlreadyExists";
    pub const INVALID_PROPERTIES: &str = "invalidProperties";
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn a_principal_round_trips_through_camel_case_json() {
        let principal = Principal {
            id: Some(Id::new("P1")),
            principal_type: Some("individual".to_owned()),
            name: "Alice Example".to_owned(),
            email: Some("alice@example.com".to_owned()),
            capabilities: BTreeMap::from([(
                "urn:ietf:params:jmap:calendars".to_owned(),
                serde_json::json!({"mayGetAvailability": true}),
            )]),
            ..Principal::default()
        };

        let json = serde_json::to_value(&principal).unwrap();
        assert_eq!(json["id"], "P1");
        assert_eq!(json["type"], "individual");
        assert_eq!(json["email"], "alice@example.com");
        assert_eq!(
            json["capabilities"]["urn:ietf:params:jmap:calendars"]["mayGetAvailability"],
            true
        );

        let round_tripped: Principal = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, principal);
    }

    #[test]
    fn an_unmodeled_property_survives_in_extra() {
        let principal: Principal = serde_json::from_value(serde_json::json!({
            "id": "P1",
            "name": "Alice",
            "somethingFuture": 42,
        }))
        .unwrap();
        assert_eq!(principal.extra.get("somethingFuture"), Some(&42.into()));

        let json = serde_json::to_value(&principal).unwrap();
        assert_eq!(json["somethingFuture"], 42);
    }

    #[test]
    fn get_availability_request_round_trips_with_camel_case_arguments() {
        let request = GetAvailabilityRequest::new(
            Id::new("A1"),
            Id::new("P1"),
            UtcDate::new("2026-09-01T00:00:00Z"),
            UtcDate::new("2026-09-02T00:00:00Z"),
        )
        .show_details();

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["accountId"], "A1");
        assert_eq!(json["id"], "P1");
        assert_eq!(json["utcStart"], "2026-09-01T00:00:00Z");
        assert_eq!(json["utcEnd"], "2026-09-02T00:00:00Z");
        assert_eq!(json["showDetails"], true);
        assert!(json.get("eventProperties").is_none());

        let round_tripped: GetAvailabilityRequest = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, request);
    }

    #[test]
    fn busy_period_round_trips_with_and_without_an_event() {
        let without_event = BusyPeriod::new(
            UtcDate::new("2026-09-01T13:00:00Z"),
            UtcDate::new("2026-09-01T14:00:00Z"),
            "confirmed",
        );
        let json = serde_json::to_value(&without_event).unwrap();
        assert!(json.get("event").is_none());
        assert_eq!(
            serde_json::from_value::<BusyPeriod>(json).unwrap(),
            without_event
        );

        let with_event = BusyPeriod::new(
            UtcDate::new("2026-09-01T13:00:00Z"),
            UtcDate::new("2026-09-01T14:00:00Z"),
            "confirmed",
        )
        .with_event(CalendarEvent {
            title: Some("Dentist".to_owned()),
            ..CalendarEvent::default()
        });
        let json = serde_json::to_value(&with_event).unwrap();
        assert_eq!(json["event"]["title"], "Dentist");
        assert_eq!(
            serde_json::from_value::<BusyPeriod>(json).unwrap(),
            with_event
        );
    }

    #[test]
    fn get_availability_response_round_trips_a_list_of_busy_periods() {
        let response = GetAvailabilityResponse {
            list: vec![BusyPeriod::new(
                UtcDate::new("2026-09-01T13:00:00Z"),
                UtcDate::new("2026-09-01T14:00:00Z"),
                "tentative",
            )],
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["list"][0]["busyStatus"], "tentative");
        assert_eq!(
            serde_json::from_value::<GetAvailabilityResponse>(json).unwrap(),
            response
        );
    }
}
