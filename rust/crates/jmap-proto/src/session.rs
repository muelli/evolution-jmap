// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session object served at `/.well-known/jmap` (RFC 8620 §2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;
use crate::state::State;

pub const CAPABILITY_CORE: &str = "urn:ietf:params:jmap:core";
pub const CAPABILITY_MAIL: &str = "urn:ietf:params:jmap:mail";
pub const CAPABILITY_SUBMISSION: &str = "urn:ietf:params:jmap:submission";
pub const CAPABILITY_CONTACTS: &str = "urn:ietf:params:jmap:contacts";
pub const CAPABILITY_CALENDARS: &str = "urn:ietf:params:jmap:calendars";

/// Server capabilities, available accounts, and endpoint URLs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub capabilities: BTreeMap<String, Value>,
    pub accounts: BTreeMap<Id, Account>,
    pub primary_accounts: BTreeMap<String, Id>,
    pub username: String,
    pub api_url: String,
    pub download_url: String,
    pub upload_url: String,
    pub event_source_url: String,
    pub state: State,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Session {
    /// The primary account id for a capability URN, if the server has one.
    pub fn primary_account(&self, capability: &str) -> Option<&Id> {
        self.primary_accounts.get(capability)
    }

    /// How many ids one `/get` call may name (RFC 8620 §2, the core
    /// capability's `maxObjectsInGet`).
    ///
    /// `None` when the server does not say — which RFC 8620 does not allow,
    /// but which a caller has to have an answer for anyway, because asking for
    /// too many is a `requestTooLarge` that fails the whole call rather than a
    /// truncated answer. What to fall back to is the caller's decision, not
    /// this type's: the limit that matters is the one for the objects it is
    /// about to ask for.
    pub fn max_objects_in_get(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxObjectsInGet")?
            .as_u64()
    }
}

/// One account the user has access to (RFC 8620 §1.6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    pub is_personal: bool,
    pub is_read_only: bool,
    pub account_capabilities: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
