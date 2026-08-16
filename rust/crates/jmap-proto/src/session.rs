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
