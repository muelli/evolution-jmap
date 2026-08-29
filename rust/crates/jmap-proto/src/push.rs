// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 8620 §7 Push and EventSource types: `PushSubscription`,
//! `PushVerification`, and `StateChange`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;
use crate::state::{State, UtcDate};

/// A push subscription object (RFC 8620 §7.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscription {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    pub device_client_id: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<PushSubscriptionKeys>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PushSubscription {
    pub fn new(device_client_id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: None,
            device_client_id: device_client_id.into(),
            url: url.into(),
            keys: None,
            expires: None,
            types: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_keys(mut self, p256dh: impl Into<String>, auth: impl Into<String>) -> Self {
        self.keys = Some(PushSubscriptionKeys::new(p256dh, auth));
        self
    }

    pub fn with_expires(mut self, expires: impl Into<UtcDate>) -> Self {
        self.expires = Some(expires.into());
        self
    }

    pub fn with_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.types = Some(types.into_iter().map(Into::into).collect());
        self
    }
}

/// Elliptic curve keys for Web Push payload encryption (RFC 8620 §7.2, RFC 8291).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PushSubscriptionKeys {
    pub p256dh: String,
    pub auth: String,
}

impl PushSubscriptionKeys {
    pub fn new(p256dh: impl Into<String>, auth: impl Into<String>) -> Self {
        Self {
            p256dh: p256dh.into(),
            auth: auth.into(),
        }
    }
}

/// A verification challenge sent by the server to validate a push subscription URL (RFC 8620 §7.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushVerification {
    #[serde(rename = "@type")]
    pub object_type: String,
    pub push_subscription_id: Id,
    pub verification_code: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PushVerification {
    pub fn new(push_subscription_id: impl Into<Id>, verification_code: impl Into<String>) -> Self {
        Self {
            object_type: "PushVerification".to_owned(),
            push_subscription_id: push_subscription_id.into(),
            verification_code: verification_code.into(),
            extra: BTreeMap::new(),
        }
    }
}

/// A state change notification delivered via EventSource or Push (RFC 8620 §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateChange {
    #[serde(rename = "@type")]
    pub object_type: String,
    pub changed: BTreeMap<Id, BTreeMap<String, State>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl StateChange {
    pub fn new(changed: BTreeMap<Id, BTreeMap<String, State>>) -> Self {
        Self {
            object_type: "StateChange".to_owned(),
            changed,
            extra: BTreeMap::new(),
        }
    }
}

/// The `SetError` types RFC 8620 §7.2.1 adds for `PushSubscription/set`.
pub mod push_subscription_set_error {
    pub const INVALID_URL: &str = "invalidUrl";
    pub const EXPIRES_TOO_FAR: &str = "expiresTooFar";
}
