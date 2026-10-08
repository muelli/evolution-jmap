// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A real `_jmap._tcp` SRV lookup for the [`Resolver`] seam `jmap-client`
//! leaves deliberately empty.
//!
//! RFC 8620 §2.2 lets a server publish its JMAP host as a `_jmap._tcp` SRV
//! record rather than answering at the bare email domain, and Fastmail does
//! exactly that, which is why a plain email+password account there fetched
//! `https://fastmail.com/.well-known/jmap` and got a 404. `jmap-client`
//! cannot do the lookup itself: it is a dependency-lean, runtime-free crate
//! and a DNS resolver would be a dependency every embedder pays for. So it
//! defines the [`Resolver`] trait and defaults it to `NoSrvResolver`, and the
//! answer lives here, in the layer that is already linked against GLib.
//!
//! The lookup is GLib's own `g_resolver_lookup_service()`, dispatched on a
//! dedicated resolver thread that owns and iterates its own GMainContext.

use std::ffi::CString;

use jmap_client::resolver::{Resolver, SrvTarget};

/// Resolves `_jmap._tcp.<domain>` through the system's DNS, via GLib's default
/// `GResolver` dispatched on a dedicated resolver thread.
///
/// This is what the EDS backends and the "Look Up Account Details" worker
/// install in place of `jmap_client::resolver::NoSrvResolver`. Anything other
/// than one usable target: no record, a lookup that fails, a name that cannot
/// even be handed to C, reads as "no record", which
/// `ClientBuilder::connect_domain` answers by trying the bare domain. That
/// direction matters: an SRV record can only ever redirect discovery, never
/// break the deployments (Stalwart, self-hosted, the in-repo mock) that answer
/// at their own domain and publish no record at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn lookup_srv(&self, domain: &str) -> Option<SrvTarget> {
        tracing::debug!(domain, "resolving _jmap._tcp SRV record");
        // A domain with an interior NUL cannot be a C string, and it cannot be
        // a real domain either: treat it as unresolvable.
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

    struct CapturingSubscriber {
        captured: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    struct Recorder<'a>(&'a std::sync::Mutex<Vec<(String, String)>>);

    impl tracing::field::Visit for Recorder<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .unwrap()
                .push((field.name().to_owned(), format!("{value:?}")));
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0
                .lock()
                .unwrap()
                .push((field.name().to_owned(), value.to_owned()));
        }
    }

    impl tracing::Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            event.record(&mut Recorder(&self.captured));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[test]
    fn lookup_srv_traces_domain_field() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };

        let _ = tracing::subscriber::with_default(subscriber, || {
            super::SystemResolver.lookup_srv("invalid.domain.invalid")
        });

        let entries = captured.lock().unwrap();
        assert!(
            entries.contains(&("domain".to_owned(), "invalid.domain.invalid".to_owned())),
            "expected domain field, got {entries:?}"
        );
    }
}
