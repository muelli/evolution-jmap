// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Authenticate requests with optional Basic and Bearer credentials.

use std::collections::BTreeMap;

use base64::Engine as _;
use jmap_proto::Id;

/// Store accepted `Authorization` header values.
/// Allow every request when this list is empty.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    accepted: Vec<String>,
    /// Principal identity for one accepted bearer token value.
    identities: BTreeMap<String, Id>,
    /// Remaining successful authorizations for accepted credentials.
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

    /// Accept `token` and bind it to `principal`.
    pub fn allow_bearer_as(&mut self, token: &str, principal: Id) {
        let header = format!("Bearer {token}");
        self.accepted.push(header.clone());
        self.identities.insert(header, principal);
    }

    /// Return the principal from [`Self::allow_bearer_as`] when present.
    pub fn identity_for(&self, authorization: Option<&str>) -> Option<Id> {
        authorization.and_then(|value| self.identities.get(value).cloned())
    }

    /// Replace accepted bearer tokens with `token`.
    /// Keep accepted Basic credentials.
    pub fn replace_bearer(&mut self, token: &str) {
        self.accepted.retain(|value| !value.starts_with("Bearer "));
        self.allow_bearer(token);
        self.remaining_uses = None;
    }

    /// Stop accepting credentials after `remaining` successful authorizations.
    pub fn expire_after(&mut self, remaining: usize) {
        self.remaining_uses = Some(remaining);
    }

    /// Validate one `Authorization` header value.
    /// Consume one remaining use on success.
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
