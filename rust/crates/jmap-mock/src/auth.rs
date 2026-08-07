// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Request authentication: configurable HTTP Basic and Bearer.

use base64::Engine as _;

/// Accepted credentials. With neither configured, every request passes.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    accepted: Vec<String>,
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

    pub fn requires_auth(&self) -> bool {
        !self.accepted.is_empty()
    }

    /// Check an `Authorization` header value (or its absence).
    pub fn authorized(&self, authorization: Option<&str>) -> bool {
        if self.accepted.is_empty() {
            return true;
        }
        authorization.is_some_and(|value| self.accepted.iter().any(|accepted| accepted == value))
    }
}
