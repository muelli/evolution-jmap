// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session object served at `/.well-known/jmap` (RFC 8620 §2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;
use crate::state::State;

/// The core capability every JMAP server supports (RFC 8620 §2).
pub const CAPABILITY_CORE: &str = "urn:ietf:params:jmap:core";
/// The Mail capability (RFC 8621 §1.3).
pub const CAPABILITY_MAIL: &str = "urn:ietf:params:jmap:mail";
/// The EmailSubmission capability (RFC 8621 §1.4).
pub const CAPABILITY_SUBMISSION: &str = "urn:ietf:params:jmap:submission";
/// The VacationResponse capability (RFC 8621 §1.5).
pub const CAPABILITY_VACATION_RESPONSE: &str = "urn:ietf:params:jmap:vacationresponse";
/// The Message Disposition Notification capability (RFC 9007 §1.3).
pub const CAPABILITY_MDN: &str = "urn:ietf:params:jmap:mdn";
/// The Contacts capability (RFC 9610 §1.3).
pub const CAPABILITY_CONTACTS: &str = "urn:ietf:params:jmap:contacts";
/// The Calendars capability (draft-ietf-jmap-calendars-33 §1.5.1).
pub const CAPABILITY_CALENDARS: &str = "urn:ietf:params:jmap:calendars";
/// The Calendar Preferences capability (draft-ietf-jmap-calendars-33 §6).
pub const CAPABILITY_CALENDAR_PREFERENCES: &str = "urn:ietf:params:jmap:calendars:preferences";
/// The Principals capability (RFC 9670 §1.3).
pub const CAPABILITY_PRINCIPALS: &str = "urn:ietf:params:jmap:principals";
/// The Principals Owner capability (RFC 9670 §1.3).
pub const CAPABILITY_PRINCIPALS_OWNER: &str = "urn:ietf:params:jmap:principals:owner";
/// The WebSocket capability (RFC 8887 §2).
pub const CAPABILITY_WEBSOCKET: &str = "urn:ietf:params:jmap:websocket";
/// The Quota capability (RFC 9425 §1.1).
pub const CAPABILITY_QUOTA: &str = "urn:ietf:params:jmap:quota";
/// The Blob management capability (RFC 9404 §1.1).
pub const CAPABILITY_BLOB: &str = "urn:ietf:params:jmap:blob";
/// The Tasks capability (draft-ietf-jmap-tasks §1.1).
pub const CAPABILITY_TASKS: &str = "urn:ietf:params:jmap:tasks";
/// The Sieve script management capability (RFC 9661 §1.2.1).
pub const CAPABILITY_SIEVE: &str = "urn:ietf:params:jmap:sieve";
/// The S/MIME signature verification capability (RFC 9219 §3).
pub const CAPABILITY_SMIME_VERIFY: &str = "urn:ietf:params:jmap:smimeverify";
/// The FileNode capability (draft-ietf-jmap-filenode §1.2).
pub const CAPABILITY_FILENODE: &str = "urn:ietf:params:jmap:filenode";
/// The RefPlus capability (draft-ietf-jmap-refplus §1.2).
pub const CAPABILITY_REFPLUS: &str = "urn:ietf:params:jmap:refplus";
/// The Metadata capability (draft-ietf-jmap-metadata §1.2).
pub const CAPABILITY_METADATA: &str = "urn:ietf:params:jmap:metadata";
/// The Mail Sharing capability (draft-ietf-jmap-mail-sharing §1.2).
pub const CAPABILITY_MAIL_SHARE: &str = "urn:ietf:params:jmap:mail:share";
/// The Principal Availability capability (draft-ietf-jmap-calendars-33 §1.5.2).
pub const CAPABILITY_PRINCIPALS_AVAILABILITY: &str = "urn:ietf:params:jmap:principals:availability";
/// The WebPush VAPID capability (RFC 9749 §6).
pub const CAPABILITY_WEBPUSH_VAPID: &str = "urn:ietf:params:jmap:webpush-vapid";
/// Cyrus vendor mail extension capability naming `Email.snoozed` (draft-ietf-extra-email-snooze).
pub const CAPABILITY_CYRUS_MAIL: &str = "https://cyrusimap.org/ns/jmap/mail";

/// Server capabilities, available accounts, and endpoint URLs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Session {
    /// The capability URNs the server supports, each mapped to its properties object.
    pub capabilities: BTreeMap<String, Value>,
    /// Every account the user has access to, keyed by account id.
    pub accounts: BTreeMap<Id, Account>,
    /// The default account id for each capability URN the user has access to.
    #[serde(default)]
    pub primary_accounts: BTreeMap<String, Id>,
    /// A human-readable name for the authenticated user.
    pub username: String,
    /// The URL method calls are submitted to.
    pub api_url: String,
    /// The URL blobs are downloaded from.
    pub download_url: String,
    /// The URL blobs are uploaded to.
    pub upload_url: String,
    /// The URL to open an `EventSource` connection to, if the server supports push.
    #[serde(default)]
    pub event_source_url: String,
    /// A string that changes whenever anything in the session object changes.
    pub state: State,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Session {
    /// Create a session with the given username, URLs and state, and no capabilities or accounts.
    pub fn new(
        username: impl Into<String>,
        api_url: impl Into<String>,
        download_url: impl Into<String>,
        upload_url: impl Into<String>,
        state: impl Into<State>,
    ) -> Self {
        Self {
            capabilities: BTreeMap::new(),
            accounts: BTreeMap::new(),
            primary_accounts: BTreeMap::new(),
            username: username.into(),
            api_url: api_url.into(),
            download_url: download_url.into(),
            upload_url: upload_url.into(),
            event_source_url: String::new(),
            state: state.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the URL to open an `EventSource` connection to.
    pub fn with_event_source_url(mut self, url: impl Into<String>) -> Self {
        self.event_source_url = url.into();
        self
    }

    /// Set the server- or draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Add one capability's properties object, replacing it if already present.
    pub fn with_capability(mut self, uri: impl Into<String>, value: Value) -> Self {
        self.capabilities.insert(uri.into(), value);
        self
    }

    /// Replace the whole capabilities map.
    pub fn with_capabilities(mut self, capabilities: BTreeMap<String, Value>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Add one account, replacing it if already present.
    pub fn with_account(mut self, id: impl Into<Id>, account: Account) -> Self {
        self.accounts.insert(id.into(), account);
        self
    }

    /// Replace the whole accounts map.
    pub fn with_accounts(mut self, accounts: BTreeMap<Id, Account>) -> Self {
        self.accounts = accounts;
        self
    }

    /// Set the default account id for one capability URN.
    pub fn with_primary_account(
        mut self,
        capability: impl Into<String>,
        id: impl Into<Id>,
    ) -> Self {
        self.primary_accounts.insert(capability.into(), id.into());
        self
    }

    /// Replace the whole primary accounts map.
    pub fn with_primary_accounts(mut self, primary_accounts: BTreeMap<String, Id>) -> Self {
        self.primary_accounts = primary_accounts;
        self
    }

    /// The primary account id for a capability URN, if the server has one.
    pub fn primary_account(&self, capability: &str) -> Option<&Id> {
        self.primary_accounts.get(capability)
    }

    /// Resolve the account serving `capability` (RFC 8620 §2).
    /// Checks `primaryAccounts` first. If omitted, falls back to the sole personal account.
    pub fn resolve_primary_account(&self, capability: &str) -> Option<&Id> {
        if !self.capabilities.contains_key(capability) {
            return None;
        }

        let id = match self.primary_accounts.get(capability) {
            Some(id) => id,
            None => self.sole_personal_account(capability)?,
        };
        let account = self.accounts.get(id)?;
        if !account.account_capabilities.contains_key(capability) {
            return None;
        }
        Some(id)
    }

    /// The sole personal account of the user offering `capability`, or `None` if zero or multiple.
    fn sole_personal_account(&self, capability: &str) -> Option<&Id> {
        let mut candidates = self.accounts.iter().filter(|(_, account)| {
            account.is_personal && account.account_capabilities.contains_key(capability)
        });
        let (id, _) = candidates.next()?;
        match candidates.next() {
            None => Some(id),
            Some(_) => None,
        }
    }

    /// Maximum object IDs allowed in a `/get` call (RFC 8620 §2 `maxObjectsInGet`).
    pub fn max_objects_in_get(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxObjectsInGet")?
            .as_u64()
    }

    /// Maximum method calls allowed in one API request (RFC 8620 §2 `maxCallsInRequest`).
    pub fn max_calls_in_request(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxCallsInRequest")?
            .as_u64()
    }

    /// Maximum request size accepted at `apiUrl` in octets (RFC 8620 §2 `maxSizeRequest`).
    pub fn max_size_request(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxSizeRequest")?
            .as_u64()
    }

    /// Maximum upload size accepted at `uploadUrl` in octets (RFC 8620 §6.1 `maxSizeUpload`).
    pub fn max_size_upload(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxSizeUpload")?
            .as_u64()
    }

    /// The maximum number of concurrent upload requests the server will take on `uploadUrl` (RFC 8620 §2).
    pub fn max_concurrent_upload(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxConcurrentUpload")?
            .as_u64()
    }

    /// The maximum number of concurrent requests the server will take on `apiUrl` (RFC 8620 §2).
    pub fn max_concurrent_requests(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxConcurrentRequests")?
            .as_u64()
    }

    /// The maximum number of objects the server will process in a single `/set` call (RFC 8620 §2).
    pub fn max_objects_in_set(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxObjectsInSet")?
            .as_u64()
    }

    /// Supported collation algorithms for sorting (RFC 8620 §2).
    pub fn collation_algorithms(&self) -> Option<Vec<String>> {
        let arr = self
            .capabilities
            .get(CAPABILITY_CORE)?
            .get("collationAlgorithms")?
            .as_array()?;
        Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
        )
    }

    /// Typed core capability struct, if present.
    pub fn core_capability(&self) -> Option<CoreCapability> {
        let val = self.capabilities.get(CAPABILITY_CORE)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed mail capability struct, if present (RFC 8621 §1.3).
    #[cfg(feature = "mail")]
    pub fn mail_capability(&self) -> Option<crate::mail::MailCapability> {
        let val = self.capabilities.get(CAPABILITY_MAIL)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed submission capability struct, if present (RFC 8621 §1.4).
    #[cfg(feature = "mail")]
    pub fn submission_capability(&self) -> Option<crate::mail::SubmissionCapability> {
        let val = self.capabilities.get(CAPABILITY_SUBMISSION)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed contacts capability struct, if present (RFC 9610 §1.3).
    #[cfg(feature = "contacts")]
    pub fn contacts_capability(&self) -> Option<crate::contacts::ContactsCapability> {
        let val = self.capabilities.get(CAPABILITY_CONTACTS)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed calendars capability struct, if present (draft-ietf-jmap-calendars-28 §1.3).
    #[cfg(feature = "calendars")]
    pub fn calendars_capability(&self) -> Option<crate::calendars::CalendarsCapability> {
        let val = self.capabilities.get(CAPABILITY_CALENDARS)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed principals capability struct, if present (RFC 9670 §1.3).
    #[cfg(feature = "principals")]
    pub fn principals_capability(&self) -> Option<crate::principals::PrincipalsCapability> {
        let val = self.capabilities.get(CAPABILITY_PRINCIPALS)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed principals owner capability struct, if present (RFC 9670 §1.3).
    #[cfg(feature = "principals")]
    pub fn principals_owner_capability(
        &self,
    ) -> Option<crate::principals::PrincipalsOwnerCapability> {
        let val = self.capabilities.get(CAPABILITY_PRINCIPALS_OWNER)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed WebSocket capability struct, if present (RFC 8887 §2).
    pub fn websocket_capability(&self) -> Option<WebSocketCapability> {
        let val = self.capabilities.get(CAPABILITY_WEBSOCKET)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed quota capability struct, if present (RFC 9425 §1.1).
    pub fn quota_capability(&self) -> Option<crate::quota::QuotaCapability> {
        let val = self.capabilities.get(CAPABILITY_QUOTA)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed blob capability struct, if present (RFC 9404 §1.1).
    pub fn blob_capability(&self) -> Option<crate::blob::BlobCapability> {
        let val = self.capabilities.get(CAPABILITY_BLOB)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed tasks capability struct, if present (draft-ietf-jmap-tasks §1.1).
    #[cfg(feature = "calendars")]
    pub fn tasks_capability(&self) -> Option<crate::tasks::TasksCapability> {
        let val = self.capabilities.get(CAPABILITY_TASKS)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed sieve capability struct, if present (RFC 9661 §1.2.1).
    pub fn sieve_capability(&self) -> Option<crate::sieve::SieveCapability> {
        let val = self.capabilities.get(CAPABILITY_SIEVE)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed MDN capability struct, if present (RFC 9007 §1.3).
    #[cfg(feature = "mail")]
    pub fn mdn_capability(&self) -> Option<crate::mail::MDNCapability> {
        let val = self.capabilities.get(CAPABILITY_MDN)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed vacation response capability struct, if present (RFC 8621 §1.5).
    #[cfg(feature = "mail")]
    pub fn vacation_response_capability(&self) -> Option<crate::mail::VacationResponseCapability> {
        let val = self.capabilities.get(CAPABILITY_VACATION_RESPONSE)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed calendar preferences capability struct, if present (draft-ietf-jmap-calendars-28 §6).
    #[cfg(feature = "calendars")]
    pub fn calendar_preferences_capability(
        &self,
    ) -> Option<crate::calendars::CalendarPreferencesCapability> {
        let val = self.capabilities.get(CAPABILITY_CALENDAR_PREFERENCES)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed S/MIME signature verification capability struct, if present (RFC 9219 §3).
    #[cfg(feature = "mail")]
    pub fn smime_verify_capability(&self) -> Option<crate::mail::SmimeVerifyCapability> {
        let val = self.capabilities.get(CAPABILITY_SMIME_VERIFY)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed FileNode capability struct, if present (draft-ietf-jmap-filenode §1.2).
    pub fn filenode_capability(&self) -> Option<crate::filenode::FileNodeCapability> {
        let val = self.capabilities.get(CAPABILITY_FILENODE)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed RefPlus capability struct, if present (draft-ietf-jmap-refplus §1.2).
    pub fn refplus_capability(&self) -> Option<RefPlusCapability> {
        let val = self.capabilities.get(CAPABILITY_REFPLUS)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed metadata capability struct, if present (draft-ietf-jmap-metadata §1.2).
    pub fn metadata_capability(&self) -> Option<crate::metadata::MetadataCapability> {
        let val = self.capabilities.get(CAPABILITY_METADATA)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed mail share capability struct, if present (draft-ietf-jmap-mail-sharing §1.2).
    #[cfg(feature = "mail")]
    pub fn mail_share_capability(&self) -> Option<crate::mail::MailShareCapability> {
        let val = self.capabilities.get(CAPABILITY_MAIL_SHARE)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed Principal availability capability struct, if present (draft-ietf-jmap-calendars-28 §1.5.2).
    #[cfg(feature = "principals")]
    pub fn principals_availability_capability(
        &self,
    ) -> Option<crate::principals::PrincipalAvailabilityCapability> {
        let val = self.capabilities.get(CAPABILITY_PRINCIPALS_AVAILABILITY)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed WebPush VAPID capability struct, if present (RFC 9749 §6).
    pub fn webpush_vapid_capability(&self) -> Option<crate::push::WebPushVapidCapability> {
        let val = self.capabilities.get(CAPABILITY_WEBPUSH_VAPID)?;
        serde_json::from_value(val.clone()).ok()
    }
}

/// RefPlus capability properties (draft-ietf-jmap-refplus §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RefPlusCapability {
    /// Whether the server supports JSON Pointer result references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_path: Option<bool>,
    /// Whether the server supports result references inside filter conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_condition: Option<bool>,
    /// Whether the server supports result references as a `/set` property value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_property: Option<bool>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl RefPlusCapability {
    /// Create a capability object with every field at its default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set whether the server supports JSON Pointer result references.
    pub fn with_json_path(mut self, json_path: bool) -> Self {
        self.json_path = Some(json_path);
        self
    }

    /// Set whether the server supports result references inside filter conditions.
    pub fn with_filter_condition(mut self, filter_condition: bool) -> Self {
        self.filter_condition = Some(filter_condition);
        self
    }

    /// Set whether the server supports result references as a `/set` property value.
    pub fn with_set_property(mut self, set_property: bool) -> Self {
        self.set_property = Some(set_property);
        self
    }

    /// Set the server- or draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// WebSocket capability properties (RFC 8887 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebSocketCapability {
    /// The URL to open the WebSocket connection to.
    pub url: String,
    /// Whether the server supports push over this WebSocket connection.
    #[serde(default)]
    pub supports_push: bool,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WebSocketCapability {
    /// Create a capability object for the given WebSocket URL, with push unsupported.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            supports_push: false,
            extra: BTreeMap::new(),
        }
    }

    /// Set whether the server supports push over this WebSocket connection.
    pub fn supports_push(mut self, supports: bool) -> Self {
        self.supports_push = supports;
        self
    }

    /// Set the server- or draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Core capability properties (RFC 8620 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CoreCapability {
    /// The maximum upload size accepted at `uploadUrl`, in octets.
    #[serde(default)]
    pub max_size_upload: u64,
    /// The maximum number of concurrent upload requests the server will take on `uploadUrl`.
    #[serde(default)]
    pub max_concurrent_upload: u64,
    /// The maximum request size accepted at `apiUrl`, in octets.
    #[serde(default)]
    pub max_size_request: u64,
    /// The maximum number of concurrent requests the server will take on `apiUrl`.
    #[serde(default)]
    pub max_concurrent_requests: u64,
    /// The maximum number of method calls allowed in one API request.
    #[serde(default)]
    pub max_calls_in_request: u64,
    /// The maximum number of object ids allowed in a single `/get` call.
    #[serde(default)]
    pub max_objects_in_get: u64,
    /// The maximum number of objects the server will process in a single `/set` call.
    #[serde(default)]
    pub max_objects_in_set: u64,
    /// The collation algorithms the server supports for sorting.
    #[serde(default)]
    pub collation_algorithms: Vec<String>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CoreCapability {
    /// Create a capability object with every field at its default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum upload size accepted at `uploadUrl`, in octets.
    pub fn with_max_size_upload(mut self, max: u64) -> Self {
        self.max_size_upload = max;
        self
    }

    /// Set the maximum number of concurrent upload requests the server will take on `uploadUrl`.
    pub fn with_max_concurrent_upload(mut self, max: u64) -> Self {
        self.max_concurrent_upload = max;
        self
    }

    /// Set the maximum request size accepted at `apiUrl`, in octets.
    pub fn with_max_size_request(mut self, max: u64) -> Self {
        self.max_size_request = max;
        self
    }

    /// Set the maximum number of concurrent requests the server will take on `apiUrl`.
    pub fn with_max_concurrent_requests(mut self, max: u64) -> Self {
        self.max_concurrent_requests = max;
        self
    }

    /// Set the maximum number of method calls allowed in one API request.
    pub fn with_max_calls_in_request(mut self, max: u64) -> Self {
        self.max_calls_in_request = max;
        self
    }

    /// Set the maximum number of object ids allowed in a single `/get` call.
    pub fn with_max_objects_in_get(mut self, max: u64) -> Self {
        self.max_objects_in_get = max;
        self
    }

    /// Set the maximum number of objects the server will process in a single `/set` call.
    pub fn with_max_objects_in_set(mut self, max: u64) -> Self {
        self.max_objects_in_set = max;
        self
    }

    /// Set the collation algorithms the server supports for sorting.
    pub fn with_collation_algorithms(
        mut self,
        algorithms: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.collation_algorithms = algorithms.into_iter().map(Into::into).collect();
        self
    }

    /// Set the server- or draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// One account the user has access to (RFC 8620 §1.6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Account {
    /// A human-readable name for this account.
    pub name: String,
    /// Whether this account belongs to the authenticated user, rather than a shared account.
    #[serde(default)]
    pub is_personal: bool,
    /// Whether the user may create, change or delete anything in this account.
    #[serde(default)]
    pub is_read_only: bool,
    /// The capability URNs this account supports, each mapped to its properties object.
    pub account_capabilities: BTreeMap<String, Value>,
    /// Server- or draft-defined properties this type does not name yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Account {
    /// Create an account with the given name, not personal, not read-only, no capabilities.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_personal: false,
            is_read_only: false,
            account_capabilities: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Set whether this account belongs to the authenticated user.
    pub fn is_personal(mut self, is_personal: bool) -> Self {
        self.is_personal = is_personal;
        self
    }

    /// Set whether the user may create, change or delete anything in this account.
    pub fn is_read_only(mut self, is_read_only: bool) -> Self {
        self.is_read_only = is_read_only;
        self
    }

    /// Add one account capability's properties object, replacing it if already present.
    pub fn with_capability(mut self, uri: impl Into<String>, value: Value) -> Self {
        self.account_capabilities.insert(uri.into(), value);
        self
    }

    /// Replace the whole account capabilities map.
    pub fn with_capabilities(mut self, capabilities: BTreeMap<String, Value>) -> Self {
        self.account_capabilities = capabilities;
        self
    }

    /// Set the server- or draft-defined properties this type does not name yet.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Maximum delay for future message submission in seconds (RFC 8621 §7.1 `maxDelayedSend`).
    pub fn max_delayed_send(&self) -> Option<u64> {
        self.account_capabilities
            .get(CAPABILITY_SUBMISSION)?
            .get("maxDelayedSend")?
            .as_u64()
    }

    /// Whether this account claims `capability` among its `accountCapabilities` (RFC 8620 §1.6.2).
    pub fn has_capability(&self, capability: &str) -> bool {
        self.account_capabilities.contains_key(capability)
    }

    /// Typed submission capability object for this account (RFC 8621 §7.1).
    #[cfg(feature = "mail")]
    pub fn submission_capability(&self) -> Option<crate::mail::SubmissionCapability> {
        let val = self.account_capabilities.get(CAPABILITY_SUBMISSION)?;
        serde_json::from_value(val.clone()).ok()
    }
}
