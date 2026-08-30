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

/// One account's changed data types and their new state tokens, as named in
/// a [`StateChange`]'s `changed` map: the type name ("Mailbox", "Email", …)
/// paired with the state string a `Foo/get` call would currently answer.
pub type TypeState = BTreeMap<String, State>;

/// A server-pushed notification that something changed (RFC 8620 §7.1),
/// delivered over the `text/event-stream` resource named by the session
/// object's `eventSourceUrl` (§7.3).
///
/// Carries no more than *that* something changed and roughly *what* — a
/// recipient compares each state string against what it already has and
/// fetches only the accounts/types that moved, via the existing `/changes`
/// methods. `@type` is required to be present and MUST be the literal string
/// `"StateChange"`; nothing else is ever pushed on this channel to confuse
/// it with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateChange {
    #[serde(rename = "@type")]
    pub kind: String,
    pub changed: BTreeMap<Id, TypeState>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl StateChange {
    /// The literal `@type` value RFC 8620 §7.1 requires.
    pub const TYPE: &'static str = "StateChange";

    /// Build a `StateChange` naming `changed` accounts/types, with `@type`
    /// set to the required [`Self::TYPE`] rather than left for a caller to
    /// get wrong.
    pub fn new(changed: BTreeMap<Id, TypeState>) -> Self {
        Self {
            kind: Self::TYPE.to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_the_required_type_marker() {
        let change = StateChange::new(BTreeMap::new());
        assert_eq!(change.kind, "StateChange");
    }
}
