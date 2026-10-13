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
    /// Optional principal identity for a bearer token.
    /// `None` means authorization without identity scoping.
    identities: BTreeMap<String, Id>,
    /// Remaining accepted requests for the current bearer token.
    /// `None` means no forced expiry.
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

    /// Accept a bearer token and bind it to `principal`.
    pub fn allow_bearer_as(&mut self, token: &str, principal: Id) {
        let header = format!("Bearer {token}");
        self.accepted.push(header.clone());
        self.identities.insert(header, principal);
    }

    /// Return the principal bound by [`Self::allow_bearer_as`], if any.
    pub fn identity_for(&self, authorization: Option<&str>) -> Option<Id> {
        authorization.and_then(|value| self.identities.get(value).cloned())
    }

    /// Replace accepted bearer tokens with `token`; keep Basic credentials.
    pub fn replace_bearer(&mut self, token: &str) {
        self.accepted.retain(|value| !value.starts_with("Bearer "));
        self.allow_bearer(token);
        self.remaining_uses = None;
    }

    /// Expire accepted bearer token(s) after `remaining` successful requests.
    pub fn expire_after(&mut self, remaining: usize) {
        self.remaining_uses = Some(remaining);
    }

    /// Check an `Authorization` header value and consume one remaining use.
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
