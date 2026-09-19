// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Request authentication: configurable HTTP Basic and Bearer.

use std::collections::BTreeMap;

use base64::Engine as _;
use jmap_proto::Id;

/// Accepted credentials. With neither configured, every request passes.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    accepted: Vec<String>,
    /// The calling principal a bearer token identifies, for the cross-account
    /// sharing checks (Track E Phase C) that need to know *who* is asking
    /// rather than merely *whether* the request is authorized. A token with
    /// no entry here (every `allow_bearer`/`allow_basic` caller, and every
    /// test that predates sharing) resolves to `None`, which callers treat
    /// as "no identity configured, full access" — the behaviour every
    /// existing test already relies on.
    identities: BTreeMap<String, Id>,
    /// How many more otherwise-accepted requests the current bearer token is
    /// honoured for, or `None` to honour it for as long as it stays on
    /// [`Self::accepted`] (every caller before [`Self::expire_after`]
    /// existed). Sits beside `accepted` rather than replacing it: a token
    /// this names is still on the list, and [`Self::replace_bearer`] clears
    /// it, since a freshly issued token is not the one the countdown was
    /// armed against.
    remaining_uses: Option<usize>,
}

impl AuthConfig {
    pub fn allow_basic(&mut self, user: &str, password: &str) {
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        self.accepted.push(format!("Basic {encoded}"));
    }

    pub fn allow_bearer(&mut self, token: &str) {
        self.accepted.push(format!("Bearer {token}"));
    }

    /// Accept a bearer token and bind it to `principal` — this credential's
    /// requests are now "asking as" that principal id, which cross-account
    /// checks compare against a shared object's `shareWith` map.
    pub fn allow_bearer_as(&mut self, token: &str, principal: Id) {
        let header = format!("Bearer {token}");
        self.accepted.push(header.clone());
        self.identities.insert(header, principal);
    }

    /// The principal `authorization` was bound to via
    /// [`Self::allow_bearer_as`], or `None` for a credential with no bound
    /// identity (every plain `allow_bearer`/`allow_basic` caller).
    pub fn identity_for(&self, authorization: Option<&str>) -> Option<Id> {
        authorization.and_then(|value| self.identities.get(value).cloned())
    }

    /// Stop accepting whichever bearer token(s) were configured before and
    /// accept only `token` from now on, as a real server does once a client's
    /// access token has expired and been refreshed. Any Basic credentials
    /// configured separately are left untouched.
    pub fn replace_bearer(&mut self, token: &str) {
        self.accepted.retain(|value| !value.starts_with("Bearer "));
        self.allow_bearer(token);
        self.remaining_uses = None;
    }

    /// Accept the currently configured bearer token(s) for only `remaining`
    /// more requests, then start refusing them as though the token had just
    /// expired.
    ///
    /// What this is for: reproducing a token that goes stale *partway
    /// through* one client-side operation made of several HTTP requests,
    /// deterministically, by counting, rather than by racing a timer against
    /// the test thread. [`Self::replace_bearer`] already covers "the token
    /// is stale for the client's very next request"; this covers "the token
    /// is stale for the Nth request from now," which a multi-request
    /// operation (an upload, then an import, then a submission) cannot
    /// otherwise land on without this mock knowing which JMAP method a
    /// request carries before its body is even parsed.
    pub fn expire_after(&mut self, remaining: usize) {
        self.remaining_uses = Some(remaining);
    }

    /// Check an `Authorization` header value (or its absence).
    ///
    /// `&mut self` because an accepted request may spend one use of
    /// [`Self::remaining_uses`]: the only reason this check is not a pure
    /// read.
    pub fn authorized(&mut self, authorization: Option<&str>) -> bool {
        if self.accepted.is_empty() {
            return true;
        }
        if !authorization
            .is_some_and(|value| self.accepted.iter().any(|accepted| accepted == value))
        {
            return false;
        }
        match self.remaining_uses {
            None => true,
            Some(0) => false,
            Some(n) => {
                self.remaining_uses = Some(n - 1);
                true
            }
        }
    }
}
