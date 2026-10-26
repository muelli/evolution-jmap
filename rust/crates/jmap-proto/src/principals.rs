// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Principals & Sharing (RFC 9670): the `Principal` object and its
//! `/get`/`/query` methods, providing the floor for scheduling
//! (`Principal/getAvailability`, draft-ietf-jmap-calendars) and per-source
//! sharing.
//!
//! Models `Principal/get`, `Principal/query`, and `ShareNotification/get`/`/query`
//! (RFC 9670 §4) for resolving principals and reading sharing notifications.

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
#[non_exhaustive]
pub struct Principal {
    /// The principal's id, or `None` for a principal not yet created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The principal's type: one of the [`principal_type`] constants.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub principal_type: Option<String>,
    /// The principal's display name.
    #[serde(default)]
    pub name: String,
    /// A longer, human-readable description of the principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The principal's primary email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The principal's IANA time zone name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// An alphanumeric secret string to authorize access to this principal (RFC 9670 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Valid methods for sending scheduling messages to this principal (RFC 9670 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_to: Option<BTreeMap<String, String>>,
    /// Server-set, per-*principal* capability bag, distinct from the
    /// account/server capability maps in `session.rs`.
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
    /// The accounts on the server this principal corresponds to, if visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<BTreeMap<Id, Value>>,
    /// Whether this principal represents the account owner themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_personal: Option<bool>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Principal {
    /// Create a principal with the given name, all other fields unset.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set the principal's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the principal's type.
    pub fn with_type(mut self, principal_type: impl Into<String>) -> Self {
        self.principal_type = Some(principal_type.into());
        self
    }

    /// Set the principal's primary email address.
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Set the principal's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the principal's IANA time zone name.
    pub fn with_time_zone(mut self, time_zone: impl Into<String>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    /// Set the secret string that authorizes access to this principal.
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
        self
    }

    /// Set the valid methods for sending scheduling messages to this principal.
    pub fn with_send_to(mut self, send_to: BTreeMap<String, String>) -> Self {
        self.send_to = Some(send_to);
        self
    }

    /// Set the per-principal capability bag.
    pub fn with_capabilities(mut self, capabilities: BTreeMap<String, Value>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Set whether this principal represents the account owner themselves.
    pub fn is_personal(mut self, is_personal: bool) -> Self {
        self.is_personal = Some(is_personal);
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Principal/query` filter (RFC 9670 §2): resolve a person by name, email,
/// or free text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PrincipalQueryFilter {
    /// Match principals whose name contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Match principals whose email contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Match principals against this free text, across multiple properties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl PrincipalQueryFilter {
    /// Create a filter that matches on email alone.
    pub fn email(email: impl Into<String>) -> Self {
        Self {
            email: Some(email.into()),
            ..Self::default()
        }
    }

    /// Set the name substring to match.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the free text to match.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
}

/// `Principal/getAvailability` (draft-ietf-jmap-calendars §2.2).
/// Bespoke request shape specifying principal ID and time range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetAvailabilityRequest {
    /// The id of the account to query availability in.
    pub account_id: Id,
    /// The id of the principal to check availability for.
    pub id: Id,
    /// The start of the time range to check, in UTC.
    pub utc_start: UtcDate,
    /// The end of the time range to check, in UTC.
    pub utc_end: UtcDate,
    /// Whether to return each busy period's event, where visible.
    #[serde(default)]
    pub show_details: bool,
    /// The `CalendarEvent` properties to return when `show_details` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_properties: Option<Vec<String>>,
}

impl GetAvailabilityRequest {
    /// Create a request for the given account, principal and time range.
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

    /// Ask for each busy period's event, where visible.
    pub fn show_details(mut self) -> Self {
        self.show_details = true;
        self
    }
}

/// Well-known principal types (RFC 9670 §2).
pub mod principal_type {
    /// A single person.
    pub const INDIVIDUAL: &str = "individual";
    /// A group of other principals.
    pub const GROUP: &str = "group";
    /// A bookable resource, such as a projector.
    pub const RESOURCE: &str = "resource";
    /// A bookable physical location, such as a meeting room.
    pub const LOCATION: &str = "location";
    /// A principal that does not fit the other well-known types.
    pub const OTHER: &str = "other";
}

/// Well-known busy status values (draft-ietf-jmap-calendars §2.2).
pub mod busy_status {
    /// The period is a confirmed, scheduled event.
    pub const CONFIRMED: &str = "confirmed";
    /// The period is a tentative, unconfirmed event.
    pub const TENTATIVE: &str = "tentative";
    /// The period is blocked for a reason other than a scheduled event.
    pub const UNAVAILABLE: &str = "unavailable";
}

/// `Principal/getAvailability` response: the merged `BusyPeriod`s in the
/// requested window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetAvailabilityResponse {
    /// The busy periods found in the requested window.
    #[serde(default)]
    pub list: Vec<BusyPeriod>,
}

impl GetAvailabilityResponse {
    /// Create a response from the given list of busy periods.
    pub fn new(list: impl IntoIterator<Item = BusyPeriod>) -> Self {
        Self {
            list: list.into_iter().collect(),
        }
    }
}

/// One busy interval (draft-ietf-jmap-calendars §2.2). `busy_status` is one
/// of `confirmed`, `tentative`, `unavailable`; `event` is populated only
/// when the request asked for `showDetails` and the caller may see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BusyPeriod {
    /// The start of this busy interval, in UTC.
    pub utc_start: UtcDate,
    /// The end of this busy interval, in UTC.
    pub utc_end: UtcDate,
    /// One of the [`busy_status`] constants.
    pub busy_status: String,
    /// The underlying event, present only when `showDetails` was set and the
    /// caller may see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<CalendarEvent>,
}

impl BusyPeriod {
    /// Create a busy period for the given time range and status.
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

    /// Set the underlying event.
    pub fn with_event(mut self, event: CalendarEvent) -> Self {
        self.event = Some(event);
        self
    }
}

/// A notification that a share was created, updated, or removed (RFC 9670 §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ShareNotification {
    /// The notification's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The time the share change happened.
    pub created: UtcDate,
    /// The principal who made the share change, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<Principal>,
    /// The type of the shared object: one of the
    /// [`share_notification_object_type`] constants.
    pub object_type: String,
    /// The id of the shared object.
    pub object_id: Id,
    /// The account the shared object lives in (RFC 9670 §4).
    /// Spelled `objectAccountId` on the wire, not `accountId`.
    pub object_account_id: Id,
    /// A display name for the shared object, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The object's access rights before this change, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_rights: Option<Value>,
    /// The object's access rights after this change, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_rights: Option<Value>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ShareNotification {
    /// Create a notification for the given time, object type, object id and
    /// owning account.
    pub fn new(
        created: impl Into<UtcDate>,
        object_type: impl Into<String>,
        object_id: impl Into<Id>,
        object_account_id: impl Into<Id>,
    ) -> Self {
        Self {
            id: None,
            created: created.into(),
            changed_by: None,
            object_type: object_type.into(),
            object_id: object_id.into(),
            object_account_id: object_account_id.into(),
            name: None,
            old_rights: None,
            new_rights: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the notification's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the principal who made the share change.
    pub fn with_changed_by(mut self, changed_by: Principal) -> Self {
        self.changed_by = Some(changed_by);
        self
    }

    /// Set a display name for the shared object.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the object's access rights before this change.
    pub fn with_old_rights(mut self, old_rights: Value) -> Self {
        self.old_rights = Some(old_rights);
        self
    }

    /// Set the object's access rights after this change.
    pub fn with_new_rights(mut self, new_rights: Value) -> Self {
        self.new_rights = Some(new_rights);
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard RFC 9670 §4 share notification object types.
pub mod share_notification_object_type {

    /// An address book was shared.
    pub const ADDRESS_BOOK: &str = "AddressBook";
    /// A calendar was shared.
    pub const CALENDAR: &str = "Calendar";
    /// A mailbox was shared.
    pub const MAILBOX: &str = "Mailbox";
}

/// Filter for `ShareNotification/query` (RFC 9670 §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ShareNotificationQueryFilter {
    /// Match notifications created after this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
    /// Match notifications created before this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    /// Match notifications for this object type: one of the
    /// [`share_notification_object_type`] constants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ShareNotificationQueryFilter {
    /// Create a filter that matches everything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the earliest creation time to match.
    pub fn with_after(mut self, after: impl Into<UtcDate>) -> Self {
        self.after = Some(after.into());
        self
    }

    /// Set the latest creation time to match.
    pub fn with_before(mut self, before: impl Into<UtcDate>) -> Self {
        self.before = Some(before.into());
        self
    }

    /// Set the object type to match.
    pub fn with_object_type(mut self, object_type: impl Into<String>) -> Self {
        self.object_type = Some(object_type.into());
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Principals capability properties (RFC 9670 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PrincipalsCapability {
    /// The largest number of principal ids the server accepts in one
    /// `Principal/get` call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_principals_per_get: Option<u64>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PrincipalsCapability {
    /// Create a `PrincipalsCapability` with every field unset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the largest accepted number of principal ids per `Principal/get`.
    pub fn with_max_principals_per_get(mut self, max: u64) -> Self {
        self.max_principals_per_get = Some(max);
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Principals owner capability properties (RFC 9670 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PrincipalsOwnerCapability {
    /// The id of the account the owning principal is visible in.
    pub account_id_for_principal: Id,
    /// The id of the principal that owns this account.
    pub principal_id: Id,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PrincipalsOwnerCapability {
    /// Create a capability for the given account and owning principal.
    pub fn new(account_id_for_principal: impl Into<Id>, principal_id: impl Into<Id>) -> Self {
        Self {
            account_id_for_principal: account_id_for_principal.into(),
            principal_id: principal_id.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the id of the account the owning principal is visible in.
    pub fn with_account_id_for_principal(mut self, account_id: impl Into<Id>) -> Self {
        self.account_id_for_principal = account_id.into();
        self
    }

    /// Set the id of the principal that owns this account.
    pub fn with_principal_id(mut self, principal_id: impl Into<Id>) -> Self {
        self.principal_id = principal_id.into();
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// The `SetError` types RFC 9670 §2 adds for `Principal/set`.
pub mod principal_set_error {
    /// The caller does not have permission to make this change.
    pub const FORBIDDEN: &str = "forbidden";
    /// A principal with the same identifying property already exists.
    pub const PRINCIPAL_ALREADY_EXISTS: &str = "principalAlreadyExists";
    /// One or more properties are invalid for this principal type.
    pub const INVALID_PROPERTIES: &str = "invalidProperties";
}

/// Principal availability calculation capability properties (draft-ietf-jmap-calendars-28 §1.5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PrincipalAvailabilityCapability {
    /// The largest time range, as an ISO 8601 duration, that
    /// `Principal/getAvailability` accepts between `utcStart` and `utcEnd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_availability_duration: Option<String>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PrincipalAvailabilityCapability {
    /// Create a `PrincipalAvailabilityCapability` with every field unset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the largest accepted availability-query time range.
    pub fn with_max_availability_duration(mut self, duration: impl Into<String>) -> Self {
        self.max_availability_duration = Some(duration.into());
        self
    }

    /// Set the draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
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
