// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP for Quotas (RFC 9425): the `Quota` data type, query filters, capability bags,
//! and standard constants.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;

/// A Quota object (RFC 9425 §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Quota {
    /// The quota's id.
    pub id: Id,
    /// The account this quota applies to, if it differs from the queried account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Id>,
    /// A human-readable name for the quota.
    #[serde(default)]
    pub name: String,
    /// The resource this quota measures, for example `octets` or `count`.
    #[serde(default)]
    pub resource_type: String,
    /// The amount of the resource currently used.
    #[serde(default)]
    pub used: u64,
    /// The amount of the resource at which the account is blocked.
    #[serde(default)]
    pub hard_limit: u64,
    /// The scope this quota applies over, for example `account` or `domain`.
    #[serde(default)]
    pub scope: String,
    /// The JMAP data types this quota covers.
    #[serde(default)]
    pub types: Vec<String>,
    /// The amount of the resource at which the server starts warning the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_limit: Option<u64>,
    /// The amount of the resource at which the server restricts some actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_limit: Option<u64>,
    /// A human-readable description of the quota.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Quota {
    /// Create a quota with its required fields; optional fields start unset.
    pub fn new(
        id: impl Into<Id>,
        name: impl Into<String>,
        resource_type: impl Into<String>,
        used: u64,
        hard_limit: u64,
        scope: impl Into<String>,
        types: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            account_id: None,
            name: name.into(),
            resource_type: resource_type.into(),
            used,
            hard_limit,
            scope: scope.into(),
            types: types.into_iter().map(Into::into).collect(),
            warn_limit: None,
            soft_limit: None,
            description: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the account this quota applies to.
    pub fn with_account_id(mut self, account_id: impl Into<Id>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    /// Set the amount of the resource at which the server starts warning the user.
    pub fn with_warn_limit(mut self, warn_limit: u64) -> Self {
        self.warn_limit = Some(warn_limit);
        self
    }

    /// Set the amount of the resource at which the server restricts some actions.
    pub fn with_soft_limit(mut self, soft_limit: u64) -> Self {
        self.soft_limit = Some(soft_limit);
        self
    }

    /// Set a human-readable description of the quota.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Quota/query` filter (RFC 9425 §4.4).
/// Matches `name` and `type` by substring, and `scope` and `resourceType` exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct QuotaQueryFilter {
    /// Match quotas whose name contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Match quotas whose resource type equals this value exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    /// Match quotas whose scope equals this value exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Match quotas whose `types` contains a type whose name contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl QuotaQueryFilter {
    /// Create an empty filter with no checks set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Match quotas whose name contains `name`.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Match quotas whose resource type equals `resource_type` exactly.
    pub fn with_resource_type(mut self, resource_type: impl Into<String>) -> Self {
        self.resource_type = Some(resource_type.into());
        self
    }

    /// Match quotas whose scope equals `scope` exactly.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Match quotas whose `types` contains a type matching `type`.
    pub fn with_type(mut self, r#type: impl Into<String>) -> Self {
        self.r#type = Some(r#type.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Quota capability properties (RFC 9425 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct QuotaCapability {
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl QuotaCapability {
    /// Create an empty capability.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard RFC 9425 quota resource types (§3.2).
pub mod quota_resource_type {
    /// A quota measured in octets (bytes).
    pub const OCTETS: &str = "octets";
    /// A quota measured in a count of objects.
    pub const COUNT: &str = "count";
}

/// Standard RFC 9425 quota scopes (§3.1).
pub mod quota_scope {
    /// A quota that applies to a single account.
    pub const ACCOUNT: &str = "account";
    /// A quota that applies across all accounts in a domain.
    pub const DOMAIN: &str = "domain";
    /// A quota that applies across the whole server.
    pub const GLOBAL: &str = "global";
}

/// Common `types` entries a `Quota` names (RFC 9425 §4.1). Values come from
/// the JMAP Types Names registry rather than a closed set.
pub mod quota_data_type {
    /// The `types` entry for mail quotas.
    pub const MAIL: &str = "Email";
    /// The `types` entry for contacts quotas.
    pub const CONTACTS: &str = "ContactCard";
    /// The `types` entry for calendar quotas.
    pub const CALENDARS: &str = "CalendarEvent";
}

/// The `SetError` types added for quotas (RFC 9425 §5).
pub mod quota_set_error {
    /// The set error type returned when an update would exceed a hard quota limit.
    pub const OVER_QUOTA: &str = "overQuota";
}
