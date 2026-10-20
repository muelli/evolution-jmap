// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A real `_jmap._tcp` SRV lookup for the [`Resolver`] seam. `jmap-client`
//! intentionally leaves this seam empty.
//!
//! RFC 8620 §2.2 lets a server publish its JMAP host as a `_jmap._tcp` SRV
//! record instead of answering at the bare email domain. Fastmail does this.
//! Thus a plain email+password account there fetched
//! `https://fastmail.com/.well-known/jmap` and got a 404. `jmap-client`
//! cannot do the lookup itself: it is a dependency-lean, runtime-free crate,
//! and every embedder would pay for a DNS resolver dependency. Thus it
//! defines the [`Resolver`] trait and defaults it to `NoSrvResolver`. The
//! implementation is here, in the layer that GLib is already linked into.
//!
//! The lookup is GLib's own `g_resolver_lookup_service()`. It runs on a
//! worker of a dedicated resolver thread pool. Each worker owns and iterates
//! its own GMainContext.

use std::ffi::CString;

use jmap_client::resolver::{Resolver, SrvTarget};

/// Resolves `_jmap._tcp.<domain>` through the system's DNS. It uses GLib's
/// default `GResolver` on a dedicated resolver worker thread.
///
/// The EDS backends and the "Look Up Account Details" worker install this
/// in place of `jmap_client::resolver::NoSrvResolver`. All results other
/// than one usable target read as "no record": no record, a failed lookup,
/// or a name that is not a valid C string. `ClientBuilder::connect_domain`
/// then tries the bare domain. This direction is important: an SRV record
/// can only redirect discovery. It can never break the deployments
/// (Stalwart, self-hosted, the in-repo mock) that answer at their own domain
/// and publish no record.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn lookup_srv(&self, domain: &str) -> Option<SrvTarget> {
        tracing::debug!(domain, "resolving _jmap._tcp SRV record");
        // A domain with an interior NUL cannot be a C string or a real
        // domain. Treat it as unresolvable.
        let domain_c = CString::new(domain).ok()?;
        let target = crate::resolver_thread::resolve_srv(c"jmap", c"tcp", domain_c);
        match &target {
            Some(target) => {
                tracing::debug!(
                    domain,
                    target_host = %target.host,
                    target_port = target.port,
                    "SRV record resolved"
                );
            }
            None => {
                tracing::debug!(domain, "no SRV record found or lookup failed");
            }
        }
        target
    }
}

pub(crate) fn srv_host(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_owned())
}

#[cfg(test)]
mod tests {
    use jmap_client::resolver::Resolver;
    use jmap_tracing_capture::capture;

    use super::srv_host;

    #[test]
    fn a_fully_qualified_target_loses_its_trailing_dot() {
        assert_eq!(
            srv_host("api.fastmail.com."),
            Some("api.fastmail.com".to_owned())
        );
    }

    #[test]
    fn a_target_that_is_already_relative_is_left_alone() {
        assert_eq!(
            srv_host("api.fastmail.com"),
            Some("api.fastmail.com".to_owned())
        );
    }

    #[test]
    fn the_root_target_means_no_service_here() {
        assert_eq!(srv_host("."), None);
        assert_eq!(srv_host(""), None);
    }

    #[test]
    fn lookup_srv_traces_domain_field() {
        let (_, entries) = capture(|| super::SystemResolver.lookup_srv("invalid.domain.invalid"));

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "domain" && value == "invalid.domain.invalid"),
            "expected domain field, got {entries:?}"
        );
    }
}
