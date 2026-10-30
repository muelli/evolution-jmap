// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The features that the UI can offer for each account, read from the JMAP
//! session document and cached.
//!
//! Each feature has two gates. The first gate is a synchronous
//! `ESource`/`CamelProvider` check ("is this account a JMAP account?"). The
//! widgets do that check. This module is the second gate: the server-side
//! facts. These facts need a session fetch.
//!
//! [`AccountFeatures`] holds the facts. [`SessionCache`] caches them for each
//! account. Only the snooze feature uses the cache now ([`shared`]). The
//! vacation page and scheduled send read the facts from their own
//! [`crate::link::AccountLink`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jmap_proto::Id;
use jmap_proto::session::{
    CAPABILITY_CYRUS_MAIL, CAPABILITY_MAIL, CAPABILITY_SIEVE, CAPABILITY_VACATION_RESPONSE, Session,
};

/// The server-side facts that the gates of one account need, in one read.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountFeatures {
    /// The JMAP account that receives the feature calls.
    pub account_id: Id,
    /// The server offers `VacationResponse/get` and `/set` (RFC 8621 §8).
    pub vacation: bool,
    /// The maximum number of seconds that the server can hold a submission:
    /// a non-zero `maxDelayedSend`. RFC 8621 §7.1 defines this value as
    /// *the* scheduled-send gate ("0 if this feature is not supported").
    ///
    /// By design, the gate does not also require FUTURERELEASE in the
    /// `submissionExtensions`. Fastmail advertises `maxDelayedSend: 44236800`
    /// with an *empty* extensions map, and accepts `HOLDFOR`. A stricter gate
    /// disables the feature on a server where it operates.
    pub max_hold: Option<u64>,
    /// The server offers `Email.snoozed`, the Cyrus vendor extension
    /// ([`jmap_proto::mail::SnoozeDetails`]). Without it, the server does not
    /// wake messages. Then snoozed messages stay in a folder that nothing
    /// empties.
    pub snooze: bool,
    /// The server offers `SieveScript/get` and `/set` (RFC 9661): both the
    /// session and the account must list the capability. A server can list a
    /// capability at the session level while withholding it from one
    /// account's `accountCapabilities` (RFC 8620 Section 1.6.2); either
    /// omission means the account has no Sieve scripts to manage.
    pub sieve: bool,
}

impl AccountFeatures {
    /// Read the gates from a session document.
    ///
    /// `None` when no account in the document resolves for the mail
    /// capability. An account that cannot hold mail has no feature to gate.
    pub fn from_session(session: &Session) -> Option<Self> {
        let account_id = session.resolve_primary_account(CAPABILITY_MAIL)?.clone();
        let account = session.accounts.get(&account_id)?;
        let max_hold = account.max_delayed_send().filter(|&seconds| seconds > 0);
        let sieve = session.capabilities.contains_key(CAPABILITY_SIEVE)
            && account.has_capability(CAPABILITY_SIEVE);
        Some(Self {
            vacation: account.has_capability(CAPABILITY_VACATION_RESPONSE),
            snooze: account.has_capability(CAPABILITY_CYRUS_MAIL),
            max_hold,
            sieve,
            account_id,
        })
    }
}

/// The time for which a fetched session document stays valid for its
/// account. After this time, the next lookup misses and the caller fetches
/// again.
///
/// Ten minutes: capabilities change when an operator changes the server
/// configuration, not for each message.
pub const DEFAULT_TTL: Duration = Duration::from_secs(600);

/// Per-account [`AccountFeatures`], keyed by `ESource` UID.
///
/// Callers give `Instant::now()` to the cache, and the cache does not read
/// the clock. Thus a test can check the TTL arithmetic without a sleep.
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

    /// The cached answer for `uid`, if it is not older than the TTL.
    pub fn lookup(&self, uid: &str, now: Instant) -> Option<AccountFeatures> {
        let entries = self.entries.lock().unwrap();
        let (features, fetched) = entries.get(uid)?;
        if now.saturating_duration_since(*fetched) > self.ttl {
            return None;
        }
        Some(features.clone())
    }

    /// Record the result of a session fetch.
    pub fn store(&self, uid: &str, features: AccountFeatures, now: Instant) {
        self.entries
            .lock()
            .unwrap()
            .insert(uid.to_owned(), (features, now));
    }

    /// Remove one account, so that the next lookup misses and the caller
    /// fetches again. A session that failed can have changed. No code in this
    /// crate calls this function now, other than the tests.
    pub fn invalidate(&self, uid: &str) {
        self.entries.lock().unwrap().remove(uid);
    }
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

/// The module-wide cache. All reader windows that ask about the same account
/// share one answer for [`DEFAULT_TTL`].
pub fn shared() -> &'static SessionCache {
    static SHARED: std::sync::OnceLock<SessionCache> = std::sync::OnceLock::new();
    SHARED.get_or_init(SessionCache::default)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A session document with one personal account. The test sets the
    /// capabilities of the account.
    fn session(account_capabilities: serde_json::Value) -> Session {
        serde_json::from_value(json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": {},
                "urn:ietf:params:jmap:mail": {},
                "urn:ietf:params:jmap:sieve": {},
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
            "urn:ietf:params:jmap:sieve": {},
        })))
        .unwrap();
        assert_eq!(features.account_id.as_str(), "A1");
        assert!(features.vacation);
        assert_eq!(features.max_hold, Some(2_592_000));
        assert!(features.snooze);
        assert!(features.sieve);

        let bare = AccountFeatures::from_session(&session(json!({
            "urn:ietf:params:jmap:mail": {},
        })))
        .unwrap();
        assert!(!bare.vacation);
        assert_eq!(bare.max_hold, None);
        assert!(!bare.snooze);
        assert!(!bare.sieve);
    }

    /// RFC 8620 Section 1.6.2 lets a server list a capability at the session
    /// level while withholding it from one account. Only the account check
    /// alone would wrongly gate the feature on for such an account.
    #[test]
    fn sieve_needs_both_the_session_and_the_account_to_list_it() {
        let account_only = AccountFeatures::from_session(&session(json!({
            "urn:ietf:params:jmap:mail": {},
            "urn:ietf:params:jmap:sieve": {},
        })))
        .unwrap();
        assert!(account_only.sieve);

        let session_without_it: Session = serde_json::from_value(json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": {},
                "urn:ietf:params:jmap:mail": {},
            },
            "accounts": {
                "A1": {
                    "name": "alice@example.com",
                    "isPersonal": true,
                    "isReadOnly": false,
                    "accountCapabilities": {
                        "urn:ietf:params:jmap:mail": {},
                        "urn:ietf:params:jmap:sieve": {},
                    },
                },
            },
            "primaryAccounts": {"urn:ietf:params:jmap:mail": "A1"},
            "username": "alice@example.com",
            "apiUrl": "https://example.com/jmap",
            "downloadUrl": "https://example.com/dl",
            "uploadUrl": "https://example.com/up",
            "state": "s1",
        }))
        .unwrap();
        assert!(
            !AccountFeatures::from_session(&session_without_it)
                .unwrap()
                .sieve,
            "the session omits the capability, so the account's own claim is not enough"
        );
    }

    /// The scheduled-send gate is only `maxDelayedSend` (RFC 8621: "MUST be
    /// 0 if this feature is not supported"). Fastmail sends a real limit with
    /// an *empty* `submissionExtensions`. Thus the extensions list must not be
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
            sieve: false,
        };
        let now = Instant::now();

        assert_eq!(cache.lookup("uid-1", now), None);
        cache.store("uid-1", features.clone(), now);
        assert_eq!(
            cache.lookup("uid-1", now + Duration::from_secs(599)),
            Some(features.clone())
        );
        assert_eq!(
            cache.lookup("uid-1", now + Duration::from_secs(600)),
            Some(features.clone()),
            "an answer exactly as old as the TTL is still valid"
        );
        assert_eq!(cache.lookup("uid-1", now + Duration::from_secs(601)), None);

        cache.store("uid-1", features.clone(), now);
        cache.invalidate("uid-1");
        assert_eq!(cache.lookup("uid-1", now), None);
        assert_eq!(cache.lookup("uid-2", now), None, "keys are per account");
    }

    /// All reader windows must see one cache. Otherwise each window fetches the
    /// session again.
    #[test]
    fn the_shared_cache_is_one_cache() {
        let features = AccountFeatures {
            account_id: Id::new("A1"),
            vacation: false,
            max_hold: Some(60),
            snooze: true,
            sieve: true,
        };
        let now = Instant::now();
        shared().store("uid-shared-test", features.clone(), now);
        assert_eq!(shared().lookup("uid-shared-test", now), Some(features));
    }
}
