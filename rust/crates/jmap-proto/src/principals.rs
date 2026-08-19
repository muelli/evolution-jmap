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

use crate::id::Id;

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
}
