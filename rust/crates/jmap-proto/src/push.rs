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
#[non_exhaustive]
pub struct PushSubscription {
    /// The subscription's id, assigned by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// A client-chosen id identifying the device this subscription is for.
    pub device_client_id: String,
    /// The URL the server posts push messages to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The Web Push encryption keys for this subscription, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<PushSubscriptionKeys>,
    /// The time after which the server stops pushing to this subscription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<UtcDate>,
    /// The data types to notify about; `None` means all types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    /// The verification code the client echoes back to confirm the subscription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_code: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PushSubscription {
    /// Create a subscription for `device_client_id` pushing to `url`.
    pub fn new(device_client_id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: None,
            device_client_id: device_client_id.into(),
            url: Some(url.into()),
            keys: None,
            expires: None,
            types: None,
            verification_code: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the subscription's id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the client-chosen device id.
    pub fn with_device_client_id(mut self, device_client_id: impl Into<String>) -> Self {
        self.device_client_id = device_client_id.into();
        self
    }

    /// Set the URL the server posts push messages to.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Set the Web Push encryption keys from their p256dh and auth values.
    pub fn with_keys(mut self, p256dh: impl Into<String>, auth: impl Into<String>) -> Self {
        self.keys = Some(PushSubscriptionKeys::new(p256dh, auth));
        self
    }

    /// Set the time after which the server stops pushing to this subscription.
    pub fn with_expires(mut self, expires: impl Into<UtcDate>) -> Self {
        self.expires = Some(expires.into());
        self
    }

    /// Set the data types to notify about.
    pub fn with_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.types = Some(types.into_iter().map(Into::into).collect());
        self
    }

    /// Set the verification code the client echoes back to confirm the subscription.
    pub fn with_verification_code(mut self, verification_code: impl Into<String>) -> Self {
        self.verification_code = Some(verification_code.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Elliptic curve keys for Web Push payload encryption (RFC 8620 §7.2, RFC 8291).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct PushSubscriptionKeys {
    /// The P-256 elliptic curve Diffie-Hellman public key, base64url-encoded.
    pub p256dh: String,
    /// The authentication secret, base64url-encoded.
    pub auth: String,
}

impl PushSubscriptionKeys {
    /// Create keys from their p256dh and auth values.
    pub fn new(p256dh: impl Into<String>, auth: impl Into<String>) -> Self {
        Self {
            p256dh: p256dh.into(),
            auth: auth.into(),
        }
    }

    /// Set the P-256 elliptic curve Diffie-Hellman public key.
    pub fn with_p256dh(mut self, p256dh: impl Into<String>) -> Self {
        self.p256dh = p256dh.into();
        self
    }

    /// Set the authentication secret.
    pub fn with_auth(mut self, auth: impl Into<String>) -> Self {
        self.auth = auth.into();
        self
    }
}

/// A verification challenge sent by the server to validate a push subscription URL (RFC 8620 §7.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PushVerification {
    /// The literal `@type` value, always `"PushVerification"`.
    #[serde(rename = "@type")]
    pub object_type: String,
    /// The id of the push subscription being verified.
    pub push_subscription_id: Id,
    /// The verification code the client must echo back to confirm the subscription.
    pub verification_code: String,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PushVerification {
    /// Create a verification for `push_subscription_id` with `verification_code`.
    pub fn new(push_subscription_id: impl Into<Id>, verification_code: impl Into<String>) -> Self {
        Self {
            object_type: "PushVerification".to_owned(),
            push_subscription_id: push_subscription_id.into(),
            verification_code: verification_code.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the id of the push subscription being verified.
    pub fn with_push_subscription_id(mut self, push_subscription_id: impl Into<Id>) -> Self {
        self.push_subscription_id = push_subscription_id.into();
        self
    }

    /// Set the verification code the client must echo back.
    pub fn with_verification_code(mut self, verification_code: impl Into<String>) -> Self {
        self.verification_code = verification_code.into();
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Maps changed data types to their new state tokens within a [`StateChange`] (RFC 8620 §7.1).
pub type TypeState = BTreeMap<String, State>;

/// A server-pushed notification that something changed (RFC 8620 §7.1),
/// delivered over EventSource (§7.3). `@type` MUST be `"StateChange"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StateChange {
    /// The literal `@type` value, always `"StateChange"`.
    #[serde(rename = "@type")]
    pub kind: String,
    /// The accounts and data types that changed, mapped to their new state tokens.
    pub changed: BTreeMap<Id, TypeState>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl StateChange {
    /// The literal `@type` value RFC 8620 §7.1 requires.
    pub const TYPE: &'static str = "StateChange";

    /// Create a `StateChange` with the given account changes and required `@type`.
    pub fn new(changed: BTreeMap<Id, TypeState>) -> Self {
        Self {
            kind: Self::TYPE.to_owned(),
            changed,
            extra: BTreeMap::new(),
        }
    }

    /// Replace the full set of changed accounts and data types.
    pub fn with_changed(mut self, changed: BTreeMap<Id, TypeState>) -> Self {
        self.changed = changed;
        self
    }

    /// Record that `type_name` changed to `state` for `account_id`.
    pub fn with_account_change(
        mut self,
        account_id: impl Into<Id>,
        type_name: impl Into<String>,
        state: impl Into<State>,
    ) -> Self {
        self.changed
            .entry(account_id.into())
            .or_default()
            .insert(type_name.into(), state.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// The `SetError` types RFC 8620 §7.2.1 adds for `PushSubscription/set`.
pub mod push_subscription_set_error {
    /// The set error type returned when the subscription's push URL is invalid.
    pub const INVALID_URL: &str = "invalidUrl";
    /// The set error type returned when `expires` is too far in the future.
    pub const EXPIRES_TOO_FAR: &str = "expiresTooFar";
}

/// Voluntary Application Server Identification (VAPID) Web Push capability properties (RFC 9749 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebPushVapidCapability {
    /// The VAPID application server key clients use to subscribe.
    pub application_server_key: String,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WebPushVapidCapability {
    /// Create a capability with the given VAPID application server key.
    pub fn new(application_server_key: impl Into<String>) -> Self {
        Self {
            application_server_key: application_server_key.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the VAPID application server key.
    pub fn with_application_server_key(
        mut self,
        application_server_key: impl Into<String>,
    ) -> Self {
        self.application_server_key = application_server_key.into();
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
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
