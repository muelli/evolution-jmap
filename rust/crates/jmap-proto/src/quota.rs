// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP for Quotas (RFC 9425): the `Quota` data type, query filters, capability bags,
//! and standard constants.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;

/// A Quota object (RFC 9425 §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Id>,
    pub name: String,
    pub resource_type: String,
    pub used: u64,
    pub limit: u64,
    pub scope: String,
    #[serde(default)]
    pub data_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Quota {
    pub fn new(
        id: impl Into<Id>,
        name: impl Into<String>,
        resource_type: impl Into<String>,
        used: u64,
        limit: u64,
        scope: impl Into<String>,
        data_types: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            account_id: None,
            name: name.into(),
            resource_type: resource_type.into(),
            used,
            limit,
            scope: scope.into(),
            data_types: data_types.into_iter().map(Into::into).collect(),
            warn_limit: None,
            soft_limit: None,
            description: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_account_id(mut self, account_id: impl Into<Id>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    pub fn with_warn_limit(mut self, warn_limit: u64) -> Self {
        self.warn_limit = Some(warn_limit);
        self
    }

    pub fn with_soft_limit(mut self, soft_limit: u64) -> Self {
        self.soft_limit = Some(soft_limit);
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Quota/query` filter (RFC 9425 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct QuotaQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_types: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl QuotaQueryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_resource_type(mut self, resource_type: impl Into<String>) -> Self {
        self.resource_type = Some(resource_type.into());
        self
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    pub fn with_data_types(
        mut self,
        data_types: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.data_types = Some(data_types.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Quota capability properties (RFC 9425 §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct QuotaCapability {
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl QuotaCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Standard RFC 9425 quota resource types (§2.1).
pub mod quota_resource_type {
    pub const OCTETS: &str = "octets";
    pub const COUNT: &str = "count";
}

/// Standard RFC 9425 quota scopes (§2.2).
pub mod quota_scope {
    pub const ACCOUNT: &str = "account";
    pub const DOMAIN: &str = "domain";
    pub const GLOBAL: &str = "global";
}

/// Standard RFC 9425 quota data types (§2.3).
pub mod quota_data_type {
    pub const MAIL: &str = "Mail";
    pub const CONTACTS: &str = "Contacts";
    pub const CALENDARS: &str = "Calendars";
}

/// The `SetError` types added for quotas (RFC 9425 §5).
pub mod quota_set_error {
    pub const OVER_QUOTA: &str = "overQuota";
}
