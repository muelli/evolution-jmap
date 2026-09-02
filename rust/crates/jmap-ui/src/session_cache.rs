// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the UI may offer per account, read from the JMAP session document and
//! cached.
//!
//! Gating happens twice. Whether an account is ours at all is a synchronous
//! `ESource`/`CamelProvider` question and stays with the widgets. This module
//! is the second level: the server-side facts, which cost a session fetch and
//! are therefore cached per account — one settings page, any number of
//! composers and shell views, one cache.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jmap_proto::Id;
use jmap_proto::session::{
    CAPABILITY_CYRUS_MAIL, CAPABILITY_MAIL, CAPABILITY_VACATION_RESPONSE, Session,
};

/// The server-side facts one account's gating needs, in one read.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountFeatures {
    /// The JMAP account the feature calls go to.
    pub account_id: Id,
    /// `VacationResponse/get` and `/set` are on offer (RFC 8621 §8).
    pub vacation: bool,
    /// The most seconds a submission may be held: a non-zero
    /// `maxDelayedSend`, which RFC 8621 §7.1 defines as *the* scheduled-send
    /// gate ("0 if this feature is not supported"). Deliberately not also
    /// FUTURERELEASE among the `submissionExtensions`: Fastmail advertises
    /// `maxDelayedSend: 44236800` beside an *empty* extensions map (checked
    /// live, 2026-09-02) and accepts `HOLDFOR` all the same, so the stricter
    /// reading would gate the feature off exactly where it works.
    pub max_hold: Option<u64>,
    /// `Email.snoozed` is on offer: the Cyrus vendor extension
    /// ([`jmap_proto::mail::SnoozeDetails`]). Without it there is no server
    /// wake-up, and offering snooze would strand messages in a folder nothing
    /// ever empties.
    pub snooze: bool,
}

impl AccountFeatures {
    /// Read the gates out of a session document.
    ///
    /// `None` when no account there can be resolved for the mail capability —
    /// an account that cannot hold mail has nothing here to gate.
    pub fn from_session(session: &Session) -> Option<Self> {
        let account_id = session.resolve_primary_account(CAPABILITY_MAIL)?.clone();
        let account = session.accounts.get(&account_id)?;
        let max_hold = account.max_delayed_send().filter(|&seconds| seconds > 0);
        Some(Self {
            vacation: account.has_capability(CAPABILITY_VACATION_RESPONSE),
            snooze: account.has_capability(CAPABILITY_CYRUS_MAIL),
            max_hold,
            account_id,
        })
    }
}

/// How long a fetched session document speaks for its account before it is
/// asked again. Ten minutes: capabilities change when an operator
/// reconfigures a server, not per message, and an error invalidates early
/// anyway.
pub const DEFAULT_TTL: Duration = Duration::from_secs(600);

/// Per-account [`AccountFeatures`], keyed by `ESource` UID.
///
/// Callers pass `Instant::now()` in rather than the cache reading the clock,
/// which is what lets the TTL arithmetic be tested without sleeping.
pub struct SessionCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, (AccountFeatures, Instant)>>,
}

impl SessionCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The cached answer for `uid`, unless it has aged past the TTL.
    pub fn lookup(&self, uid: &str, now: Instant) -> Option<AccountFeatures> {
        let entries = self.entries.lock().unwrap();
        let (features, fetched) = entries.get(uid)?;
        if now.saturating_duration_since(*fetched) > self.ttl {
            return None;
        }
        Some(features.clone())
    }

    /// Record what a session fetch just learned.
    pub fn store(&self, uid: &str, features: AccountFeatures, now: Instant) {
        self.entries
            .lock()
            .unwrap()
            .insert(uid.to_owned(), (features, now));
    }

    /// Forget one account, so the next ask refetches: called on any client
    /// error, since a session that just failed is one that may have changed.
    pub fn invalidate(&self, uid: &str) {
        self.entries.lock().unwrap().remove(uid);
    }
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A session document with one personal account whose capabilities are
    /// the test's to choose.
    fn session(account_capabilities: serde_json::Value) -> Session {
        serde_json::from_value(json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": {},
                "urn:ietf:params:jmap:mail": {},
            },
            "accounts": {
                "A1": {
                    "name": "alice@example.com",
                    "isPersonal": true,
                    "isReadOnly": false,
                    "accountCapabilities": account_capabilities,
                },
            },
            "primaryAccounts": {"urn:ietf:params:jmap:mail": "A1"},
            "username": "alice@example.com",
            "apiUrl": "https://example.com/jmap",
            "downloadUrl": "https://example.com/dl",
            "uploadUrl": "https://example.com/up",
            "state": "s1",
        }))
        .unwrap()
    }

    #[test]
    fn every_gate_reads_its_own_capability() {
        let features = AccountFeatures::from_session(&session(json!({
            "urn:ietf:params:jmap:mail": {},
            "urn:ietf:params:jmap:vacationresponse": {},
            "urn:ietf:params:jmap:submission": {
                "maxDelayedSend": 2592000,
                "submissionExtensions": {"FUTURERELEASE": []},
            },
            "https://cyrusimap.org/ns/jmap/mail": {},
        })))
        .unwrap();
        assert_eq!(features.account_id.as_str(), "A1");
        assert!(features.vacation);
        assert_eq!(features.max_hold, Some(2_592_000));
        assert!(features.snooze);

        let bare = AccountFeatures::from_session(&session(json!({
            "urn:ietf:params:jmap:mail": {},
        })))
        .unwrap();
        assert!(!bare.vacation);
        assert_eq!(bare.max_hold, None);
        assert!(!bare.snooze);
    }

    /// The scheduled-send gate is `maxDelayedSend` alone (RFC 8621: "MUST be
    /// 0 if this feature is not supported"): Fastmail pairs a real limit with
    /// an *empty* `submissionExtensions`, so the extensions list must not be
    /// a second requirement.
    #[test]
    fn scheduled_send_gates_on_the_limit_alone() {
        let fastmail_shape = session(json!({
            "urn:ietf:params:jmap:mail": {},
            "urn:ietf:params:jmap:submission": {
                "maxDelayedSend": 44236800,
                "submissionExtensions": {},
            },
        }));
        assert_eq!(
            AccountFeatures::from_session(&fastmail_shape)
                .unwrap()
                .max_hold,
            Some(44_236_800)
        );

        let zero_limit = session(json!({
            "urn:ietf:params:jmap:mail": {},
            "urn:ietf:params:jmap:submission": {
                "maxDelayedSend": 0,
                "submissionExtensions": {"FUTURERELEASE": []},
            },
        }));
        assert_eq!(
            AccountFeatures::from_session(&zero_limit).unwrap().max_hold,
            None
        );
    }

    #[test]
    fn a_session_without_a_mail_account_gates_nothing() {
        let no_mail: Session = serde_json::from_value(json!({
            "capabilities": {"urn:ietf:params:jmap:core": {}},
            "accounts": {},
            "username": "alice@example.com",
            "apiUrl": "https://example.com/jmap",
            "downloadUrl": "https://example.com/dl",
            "uploadUrl": "https://example.com/up",
            "state": "s1",
        }))
        .unwrap();
        assert_eq!(AccountFeatures::from_session(&no_mail), None);
    }

    #[test]
    fn the_cache_answers_within_the_ttl_and_forgets_after_it() {
        let cache = SessionCache::new(Duration::from_secs(600));
        let features = AccountFeatures {
            account_id: Id::new("A1"),
            vacation: true,
            max_hold: None,
            snooze: false,
        };
        let now = Instant::now();

        assert_eq!(cache.lookup("uid-1", now), None);
        cache.store("uid-1", features.clone(), now);
        assert_eq!(
            cache.lookup("uid-1", now + Duration::from_secs(599)),
            Some(features.clone())
        );
        assert_eq!(cache.lookup("uid-1", now + Duration::from_secs(601)), None);

        cache.store("uid-1", features.clone(), now);
        cache.invalidate("uid-1");
        assert_eq!(cache.lookup("uid-1", now), None);
        assert_eq!(cache.lookup("uid-2", now), None, "keys are per account");
    }
}
