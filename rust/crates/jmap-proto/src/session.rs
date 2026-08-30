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
pub const CAPABILITY_VACATION_RESPONSE: &str = "urn:ietf:params:jmap:vacationresponse";
pub const CAPABILITY_MDN: &str = "urn:ietf:params:jmap:mdn";
pub const CAPABILITY_CONTACTS: &str = "urn:ietf:params:jmap:contacts";
pub const CAPABILITY_CALENDARS: &str = "urn:ietf:params:jmap:calendars";
pub const CAPABILITY_CALENDAR_PREFERENCES: &str = "urn:ietf:params:jmap:calendars:preferences";
pub const CAPABILITY_PRINCIPALS: &str = "urn:ietf:params:jmap:principals";
pub const CAPABILITY_PRINCIPALS_OWNER: &str = "urn:ietf:params:jmap:principals:owner";
pub const CAPABILITY_WEBSOCKET: &str = "urn:ietf:params:jmap:websocket";
pub const CAPABILITY_QUOTA: &str = "urn:ietf:params:jmap:quota";
pub const CAPABILITY_BLOB: &str = "urn:ietf:params:jmap:blob";
pub const CAPABILITY_TASKS: &str = "urn:ietf:params:jmap:tasks";
pub const CAPABILITY_SIEVE: &str = "urn:ietf:params:jmap:sieve";
pub const CAPABILITY_SMIME_VERIFY: &str = "urn:ietf:params:jmap:smimeverify";
pub const CAPABILITY_FILENODE: &str = "urn:ietf:params:jmap:filenode";
pub const CAPABILITY_REFPLUS: &str = "urn:ietf:params:jmap:refplus";
pub const CAPABILITY_METADATA: &str = "urn:ietf:params:jmap:metadata";
pub const CAPABILITY_MAIL_SHARE: &str = "urn:ietf:params:jmap:mail:share";

/// Server capabilities, available accounts, and endpoint URLs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub capabilities: BTreeMap<String, Value>,
    pub accounts: BTreeMap<Id, Account>,
    #[serde(default)]
    pub primary_accounts: BTreeMap<String, Id>,
    pub username: String,
    pub api_url: String,
    pub download_url: String,
    pub upload_url: String,
    #[serde(default)]
    pub event_source_url: String,
    pub state: State,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Session {
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

    pub fn with_event_source_url(mut self, url: impl Into<String>) -> Self {
        self.event_source_url = url.into();
        self
    }

    pub fn with_capability(mut self, uri: impl Into<String>, value: Value) -> Self {
        self.capabilities.insert(uri.into(), value);
        self
    }

    pub fn with_capabilities(mut self, capabilities: BTreeMap<String, Value>) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_account(mut self, id: impl Into<Id>, account: Account) -> Self {
        self.accounts.insert(id.into(), account);
        self
    }

    pub fn with_primary_account(
        mut self,
        capability: impl Into<String>,
        id: impl Into<Id>,
    ) -> Self {
        self.primary_accounts.insert(capability.into(), id.into());
        self
    }

    /// The primary account id for a capability URN, if the server has one.
    pub fn primary_account(&self, capability: &str) -> Option<&Id> {
        self.primary_accounts.get(capability)
    }

    /// Which account serves `capability`, resolved the way RFC 8620 actually
    /// allows rather than by `primaryAccounts` alone.
    ///
    /// `None` when `capability` names no server capability at all (a `using`
    /// naming it would be answered `unknownCapability`, so nothing behind it
    /// is reachable), or when nothing in the document can be believed there —
    /// but before giving up, two more sources are tried:
    ///
    /// - `primaryAccounts` (RFC 8620 §2), taken as given when it has an entry
    ///   — including one naming an account outside [`Account::is_personal`],
    ///   because a server that designates a shared account as primary has
    ///   said something deliberate.
    /// - Failing that, §2 permits a server to omit `primaryAccounts`
    ///   outright ("a server that does not support this concept MUST omit
    ///   this property"), so the account is inferred from a position where
    ///   there is nothing to guess: exactly one of the user's own accounts
    ///   (`isPersonal`) offers the capability. Two of them and the answer is
    ///   `None` — guessing wrong is worse than admitting the document does
    ///   not say.
    ///
    /// Either way, a `primaryAccounts` entry naming an account that is absent
    /// from `accounts`, or one that does not itself claim the capability, is
    /// a contradiction in the document and is not believed.
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

    /// The one account of the user's own that offers `capability` — `None`
    /// when there is none, or more than one to choose between.
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

    /// How many method calls one request to `apiUrl` may carry (RFC 8620 §2,
    /// the core capability's `maxCallsInRequest`).
    ///
    /// The number that decides whether two calls chained through a
    /// back-reference may travel together. Over it, the whole request is
    /// refused with `urn:ietf:params:jmap:error:limit` — not the last call
    /// alone — so a client that chains without asking loses the calls that
    /// would have been fine.
    ///
    /// `None` when the server does not say, like the other two limits here:
    /// what to do without a number is the caller's decision. Both guesses are
    /// wrong in their own direction — a low one splits requests the server
    /// would have taken whole, a high one sends requests it refuses.
    pub fn max_calls_in_request(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxCallsInRequest")?
            .as_u64()
    }

    /// The largest request the server will take at `apiUrl`, in octets (RFC
    /// 8620 §2, the core capability's `maxSizeRequest`).
    ///
    /// The sibling of [`Self::max_calls_in_request`], counting octets where
    /// that one counts calls, and refused the same way: over it the server
    /// answers `urn:ietf:params:jmap:error:limit` and *nothing* in the request
    /// runs. A client that never asks finds out by sending the request, which
    /// for a long list of ids is the whole cost of the operation spent on an
    /// answer that was in the session document all along.
    ///
    /// `None` when the server does not say, as for the other three limits. A
    /// number invented here would split requests the server would have taken
    /// whole — and a split request is not merely slower, it has a window in it
    /// where another client can change what the first half found.
    pub fn max_size_request(&self) -> Option<u64> {
        self.capabilities
            .get(CAPABILITY_CORE)?
            .get("maxSizeRequest")?
            .as_u64()
    }

    /// The largest file the server will take on `uploadUrl`, in octets (RFC
    /// 8620 §6.1, the core capability's `maxSizeUpload`).
    ///
    /// `None` when the server does not say — which RFC 8620 §2 does not allow,
    /// and which is again the caller's decision rather than this type's: there
    /// is no safe number to invent here, because a limit made up locally would
    /// refuse uploads the server would have taken, and would be this crate's
    /// number appearing in front of a user as the account's.
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
    pub fn mail_capability(&self) -> Option<crate::mail::MailCapability> {
        let val = self.capabilities.get(CAPABILITY_MAIL)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed submission capability struct, if present (RFC 8621 §1.4).
    pub fn submission_capability(&self) -> Option<crate::mail::SubmissionCapability> {
        let val = self.capabilities.get(CAPABILITY_SUBMISSION)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed contacts capability struct, if present (RFC 9610 §1.3).
    pub fn contacts_capability(&self) -> Option<crate::contacts::ContactsCapability> {
        let val = self.capabilities.get(CAPABILITY_CONTACTS)?;
        serde_json::from_value(val.clone()).ok()
    }

    /// Typed calendars capability struct, if present (draft-ietf-jmap-calendars-28 §1.3).
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

    /// Typed sieve capability struct, if present (RFC 9265 §1.1).
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
}

/// RefPlus capability properties (draft-ietf-jmap-refplus §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RefPlusCapability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_path: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_condition: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_property: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl RefPlusCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_json_path(mut self, json_path: bool) -> Self {
        self.json_path = Some(json_path);
        self
    }

    pub fn with_filter_condition(mut self, filter_condition: bool) -> Self {
        self.filter_condition = Some(filter_condition);
        self
    }

    pub fn with_set_property(mut self, set_property: bool) -> Self {
        self.set_property = Some(set_property);
        self
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// WebSocket capability properties (RFC 8887 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketCapability {
    pub url: String,
    #[serde(default)]
    pub supports_push: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl WebSocketCapability {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            supports_push: false,
            extra: BTreeMap::new(),
        }
    }

    pub fn supports_push(mut self, supports: bool) -> Self {
        self.supports_push = supports;
        self
    }
}

/// Core capability properties (RFC 8620 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CoreCapability {
    #[serde(default)]
    pub max_size_upload: u64,
    #[serde(default)]
    pub max_concurrent_upload: u64,
    #[serde(default)]
    pub max_size_request: u64,
    #[serde(default)]
    pub max_concurrent_requests: u64,
    #[serde(default)]
    pub max_calls_in_request: u64,
    #[serde(default)]
    pub max_objects_in_get: u64,
    #[serde(default)]
    pub max_objects_in_set: u64,
    #[serde(default)]
    pub collation_algorithms: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CoreCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_size_upload(mut self, max: u64) -> Self {
        self.max_size_upload = max;
        self
    }

    pub fn with_max_concurrent_upload(mut self, max: u64) -> Self {
        self.max_concurrent_upload = max;
        self
    }

    pub fn with_max_size_request(mut self, max: u64) -> Self {
        self.max_size_request = max;
        self
    }

    pub fn with_max_concurrent_requests(mut self, max: u64) -> Self {
        self.max_concurrent_requests = max;
        self
    }

    pub fn with_max_calls_in_request(mut self, max: u64) -> Self {
        self.max_calls_in_request = max;
        self
    }

    pub fn with_max_objects_in_get(mut self, max: u64) -> Self {
        self.max_objects_in_get = max;
        self
    }

    pub fn with_max_objects_in_set(mut self, max: u64) -> Self {
        self.max_objects_in_set = max;
        self
    }

    pub fn with_collation_algorithms(
        mut self,
        algorithms: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.collation_algorithms = algorithms.into_iter().map(Into::into).collect();
        self
    }
}

/// One account the user has access to (RFC 8620 §1.6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    #[serde(default)]
    pub is_personal: bool,
    #[serde(default)]
    pub is_read_only: bool,
    pub account_capabilities: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Account {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_personal: false,
            is_read_only: false,
            account_capabilities: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    pub fn is_personal(mut self, is_personal: bool) -> Self {
        self.is_personal = is_personal;
        self
    }

    pub fn is_read_only(mut self, is_read_only: bool) -> Self {
        self.is_read_only = is_read_only;
        self
    }

    pub fn with_capability(mut self, uri: impl Into<String>, value: Value) -> Self {
        self.account_capabilities.insert(uri.into(), value);
        self
    }

    pub fn with_capabilities(mut self, capabilities: BTreeMap<String, Value>) -> Self {
        self.account_capabilities = capabilities;
        self
    }

    /// The furthest into the future an `EmailSubmission`'s `sendAt` may be set
    /// (RFC 8621 §7.1, the submission account capability's `maxDelayedSend`,
    /// in seconds), backed server-side by SMTP FUTURERELEASE (RFC 4865).
    ///
    /// `None` means the server did not name a limit for this account — which
    /// covers two different server shapes a caller cannot tell apart from
    /// this alone: no delayed send at all (`maxDelayedSend` absent, or `0`
    /// per RFC 8621's own "MUST be 0 if this feature is not supported"), and
    /// an account that does not offer the submission capability in the first
    /// place. Either way, the answer for `docs/ROADMAP.md` item 29's "detect
    /// server support" is the same: do not offer scheduled send.
    pub fn max_delayed_send(&self) -> Option<u64> {
        self.account_capabilities
            .get(CAPABILITY_SUBMISSION)?
            .get("maxDelayedSend")?
            .as_u64()
    }
}
