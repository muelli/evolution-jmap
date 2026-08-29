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

/// Elliptic curve keys for Web Push payload encryption (RFC 8620 §7.2, RFC 8291).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PushSubscriptionKeys {
    pub p256dh: String,
    pub auth: String,
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

/// The `SetError` types RFC 8620 §7.2.1 adds for `PushSubscription/set`.
pub mod push_subscription_set_error {
    pub const INVALID_URL: &str = "invalidUrl";
    pub const EXPIRES_TOO_FAR: &str = "expiresTooFar";
}
