// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 8620 §2.2 SRV autodiscovery: `_jmap._tcp.<domain>`.
//!
//! A client that knows only an email address's domain normally fetches
//! `https://<domain>/.well-known/jmap` directly. Some deployments (Fastmail
//! among them — see `docs/NIGHT-LOG.md`, "JMAP SRV autodiscovery") instead
//! publish the real JMAP host via a `_jmap._tcp` SRV record and answer
//! nothing at the bare domain, so a client that only ever tries the bare
//! domain gets a 404 for those.
//!
//! This crate does no DNS itself — deliberately: adding a DNS resolver crate
//! here would cost every embedder a dependency most of them do not need.
//! [`NoSrvResolver`] is the default and matches today's bare-domain-only
//! behaviour; an embedder able to perform a real lookup (the EDS
//! integration, via `g_resolver_lookup_service()`) supplies its own
//! [`Resolver`] via [`crate::ClientBuilder::resolver`].

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

/// The default resolver: no SRV support, ever. Every domain falls back to
/// today's bare-domain session discovery.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSrvResolver;

impl Resolver for NoSrvResolver {
    fn lookup_srv(&self, _domain: &str) -> Option<SrvTarget> {
        None
    }
}
