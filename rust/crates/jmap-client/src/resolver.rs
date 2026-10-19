// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 8620 Section 2.2 SRV autodiscovery: `_jmap._tcp.<domain>`.
//!
//! Clients can locate JMAP servers via SRV records when the apex domain returns no service.
//!
//! This crate does not bundle a DNS resolver dependency.
//! [`NoSrvResolver`] provides the default fallback behavior.
//! Embedders provide custom resolvers via [`crate::ClientBuilder::resolver`].

/// The host and port an SRV lookup resolved a domain to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvTarget {
    pub host: String,
    pub port: u16,
}

/// Looks up the JMAP SRV record for a domain.
pub trait Resolver: Send + Sync {
    /// Returns the SRV target for `domain`, or `None` if there is no record
    /// (or the resolver cannot look one up), meaning "try the bare domain".
    fn lookup_srv(&self, domain: &str) -> Option<SrvTarget>;
}

/// Default resolver that performs no SRV lookups and falls back to apex domains.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSrvResolver;

impl Resolver for NoSrvResolver {
    fn lookup_srv(&self, _domain: &str) -> Option<SrvTarget> {
        None
    }
}
