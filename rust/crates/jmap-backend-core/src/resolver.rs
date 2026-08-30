// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A real `_jmap._tcp` SRV lookup for the [`Resolver`] seam `jmap-client`
//! leaves deliberately empty.
//!
//! RFC 8620 §2.2 lets a server publish its JMAP host as a `_jmap._tcp` SRV
//! record rather than answering at the bare email domain, and Fastmail does
//! exactly that — which is why a plain email+password account there fetched
//! `https://fastmail.com/.well-known/jmap` and got a 404. `jmap-client`
//! cannot do the lookup itself: it is a dependency-lean, runtime-free crate
//! and a DNS resolver would be a dependency every embedder pays for. So it
//! defines the [`Resolver`] trait and defaults it to `NoSrvResolver`, and the
//! answer lives here, in the layer that is already linked against GLib.
//!
//! The lookup is GLib's own: `g_resolver_lookup_service()`, bound in `gio-sys`
//! and part of frozen GIO API since 2.22. That is the whole reason to do it
//! here rather than reach for a DNS crate — no new dependency, no
//! hand-written DNS packet parsing, and no async runtime dragged into a
//! deliberately blocking workspace. GLib also does two things this module
//! would otherwise have to: it returns the targets already sorted into RFC
//! 2782 preference order (so the first one is the one to use), and it frees
//! the whole list through one function.

use std::ffi::{CStr, CString};
use std::ptr;

use gio_sys::{
    GSrvTarget, g_resolver_free_targets, g_resolver_get_default, g_resolver_lookup_service,
    g_srv_target_get_hostname, g_srv_target_get_port,
};
use glib_sys::{GError, g_error_free};
use gobject_sys::g_object_unref;
use jmap_client::resolver::{Resolver, SrvTarget};

/// Resolves `_jmap._tcp.<domain>` through the system's DNS, via GLib's default
/// `GResolver`.
///
/// This is what the EDS backends and the "Look Up Account Details" worker
/// install in place of `jmap_client::resolver::NoSrvResolver`. Anything other
/// than one usable target — no record, a lookup that fails, a name that cannot
/// even be handed to C — reads as "no record", which
/// `ClientBuilder::connect_domain` answers by trying the bare domain. That
/// direction matters: an SRV record can only ever *redirect* discovery, never
/// break the deployments (Stalwart, self-hosted, the in-repo mock) that answer
/// at their own domain and publish no record at all.
///
/// The lookup blocks the calling thread and is not cancellable — the
/// [`Resolver`] trait passes no `GCancellable`, so a NULL one is handed to
/// GLib. Both call sites already run on a worker thread and the system
/// resolver applies its own timeout, so the cost of that is a connect attempt
/// that cannot be interrupted during DNS; adding a cancellable would mean
/// storing a raw `GCancellable` pointer in a `Send + Sync` value, which is a
/// worse trade for a lookup this short.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn lookup_srv(&self, domain: &str) -> Option<SrvTarget> {
        tracing::debug!(domain, "resolving _jmap._tcp SRV record");
        // A domain with an interior NUL cannot be a C string, and it cannot be
        // a real domain either — treat it as unresolvable rather than as an
        // error the vfunc above has to have a story for.
        let domain_c = CString::new(domain).ok()?;
        let target = lookup_first_service_target(c"jmap", c"tcp", &domain_c);
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

/// The first target GLib returns for `_<service>._<protocol>.<domain>`, or
/// `None` if there is not one to be had.
///
/// All the ownership lives in this one function, deliberately: GLib hands out
/// two things that must come back — a `GResolver` reference and a
/// `GSrvTarget` list — and one thing that must not be kept, the hostname
/// pointer inside a target, which the list's own free function invalidates.
/// Keeping the whole borrow-and-return inside a single body is what makes the
/// order (read every field into owned Rust data, *then* free) checkable by
/// reading it.
fn lookup_first_service_target(
    service: &CStr,
    protocol: &CStr,
    domain: &CStr,
) -> Option<SrvTarget> {
    // SAFETY: no arguments; returns a strong reference (transfer full) or, in
    // principle, NULL — unrefed below on every path.
    let resolver = unsafe { g_resolver_get_default() };
    if resolver.is_null() {
        return None;
    }

    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: `resolver` is a live `GResolver`; the three names are valid
    // NUL-terminated strings for the duration of the call; a NULL
    // `GCancellable` is explicitly allowed; `error` is a writable
    // `GError *` slot, initialised to NULL as the GError convention requires.
    // The returned list is transfer-full — freed below, on both the found and
    // the not-found path.
    let targets = unsafe {
        g_resolver_lookup_service(
            resolver,
            service.as_ptr(),
            protocol.as_ptr(),
            domain.as_ptr(),
            ptr::null_mut(),
            &mut error,
        )
    };

    // Read before freeing: the hostname belongs to the target, which
    // `g_resolver_free_targets` destroys along with the list. GLib documents
    // the list as non-empty on success and NULL on failure, and as already
    // sorted into RFC 2782 order, so the first node is the target to use.
    let first = if targets.is_null() {
        None
    } else {
        // SAFETY: `targets` is a live `GList` whose nodes' `data` are
        // `GSrvTarget *`, per `g_resolver_lookup_service`'s documented
        // element type.
        unsafe { read_target((*targets).data.cast::<GSrvTarget>()) }
    };

    if !targets.is_null() {
        // SAFETY: `targets` is the transfer-full list from the call above,
        // not yet freed and no longer read from.
        unsafe { g_resolver_free_targets(targets) };
    }
    if !error.is_null() {
        // SAFETY: `error` was written by the failing call above and is owned
        // by us; nothing else refers to it.
        unsafe { g_error_free(error) };
    }
    // SAFETY: balances the strong reference `g_resolver_get_default` returned.
    unsafe { g_object_unref(resolver.cast()) };

    first
}

/// One `GSrvTarget` as an owned [`SrvTarget`], or `None` if it does not name a
/// host and port worth connecting to.
///
/// # Safety
///
/// `target` must be NULL or a live `GSrvTarget` that outlives the call.
unsafe fn read_target(target: *mut GSrvTarget) -> Option<SrvTarget> {
    if target.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a live target; the hostname is
    // transfer-none, owned by the target, so it is copied rather than kept.
    let host = unsafe { g_srv_target_get_hostname(target) };
    if host.is_null() {
        return None;
    }
    // SAFETY: as above — a valid NUL-terminated string owned by the target.
    let host = srv_host(&unsafe { CStr::from_ptr(host) }.to_string_lossy())?;

    // SAFETY: as above.
    let port = unsafe { g_srv_target_get_port(target) };
    if port == 0 {
        return None;
    }

    Some(SrvTarget { host, port })
}

/// An SRV target's hostname as something a URL can be built from, or `None`
/// if it is RFC 2782's way of saying there is no service here.
///
/// Two shapes to normalise, both of which would otherwise reach
/// `ClientBuilder::connect_domain` and be pasted into a URL as-is:
///
/// - a **trailing dot**. DNS names are fully qualified and a resolver may
///   present one that way; `https://api.example.com.` is not wrong so much as
///   needlessly unlike the host every certificate, log line and comparison
///   elsewhere uses.
/// - a target of **`.`** alone, which RFC 2782 §3 defines as "the service is
///   decidedly not available at this domain". Answering `None` sends the
///   caller to the bare-domain fallback, which is a weaker reading of the
///   record than the RFC's — but the alternative is refusing to connect an
///   account whose provider serves JMAP at its own domain and published a
///   stale record, and a 404 there is a better failure than a setup that
///   cannot be attempted at all.
fn srv_host(host: &str) -> Option<String> {
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
