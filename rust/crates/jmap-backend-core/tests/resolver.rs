// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The real `_jmap._tcp` SRV resolver, against the system's real DNS.
//
// `jmap-client` supplies only the seam and a `NoSrvResolver` that never finds
// a record. Without a real `Resolver`, a plain email+password account fetches
// `https://<email domain>/.well-known/jmap`. That gives a 404 for a provider
// that publishes JMAP through SRV (Fastmail). These tests cover the part that
// is deterministic without a network. A domain with no record answers "none".
// That answer keeps the bare-domain fallback, and thus every self-hosted
// deployment. Also, the FFI call and the input conversion do not panic or
// fail when repeated.
//
// The success path needs a domain that really publishes a `_jmap._tcp` record.
// Thus its test is `#[ignore]`d, because it is not hermetic. To run it, use
// `cargo test -p jmap-backend-core --test resolver -- --ignored`.

use jmap_backend_core::resolver::SystemResolver;
use jmap_client::resolver::Resolver;

/// RFC 6761 reserves `.invalid` so that a name under it never resolves. Thus
/// the result is deterministic: with or without DNS egress, the lookup fails.
/// The answer must be "no record", not an error that the caller must handle.
/// A failed SRV lookup means "try the bare domain". Every deployment that
/// answers at its own domain (Stalwart, self-hosted, and the mock) needs this.
#[test]
fn a_domain_with_no_srv_record_reports_none_so_the_bare_domain_is_tried() {
    assert_eq!(
        SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
        None
    );
}

/// C gets a domain as a NUL-terminated string, so a domain with an interior
/// NUL cannot go to C. The domain comes from an `ESource` keyfile that a user
/// can write by hand. Thus a failed conversion must give "no record", as for
/// any other unresolvable name. It must not panic inside a vfunc.
#[test]
fn a_domain_with_an_interior_nul_reports_none_rather_than_panicking() {
    assert_eq!(SystemResolver.lookup_srv("exa\0mple.com"), None);
    assert_eq!(SystemResolver.lookup_srv("\0"), None);
}

/// Each lookup gets a `GResolver` reference and a `GSrvTarget` list from GLib
/// and must release both. An error here is not visible in one call. It shows
/// as a leak or a crash only in real use, where a backend reconnects for the
/// lifetime of an Evolution session. Thus the test repeats the failing path
/// (no list, but an error) until an unbalanced reference or free becomes
/// visible.
#[test]
fn repeated_lookups_are_stable() {
    for _ in 0..64 {
        assert_eq!(
            SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
            None
        );
    }
}

/// The success path, against the provider whose 404 caused this work. It is
/// not hermetic: it needs DNS egress and asserts a third party's live DNS
/// record. Thus it is ignored by default and you must run it manually. It
/// proves what no fake can: the code walks and reads the `GSrvTarget` list
/// from GLib correctly. It also normalises the host into a value from which
/// `ClientBuilder::connect_domain` can build a URL (no trailing dot).
#[test]
#[ignore = "needs DNS egress and asserts a third party's live SRV record"]
fn fastmail_publishes_its_jmap_host_via_srv() {
    let target = SystemResolver
        .lookup_srv("fastmail.com")
        .expect("_jmap._tcp.fastmail.com publishes an SRV record");

    assert_eq!(target.host, "api.fastmail.com");
    assert_eq!(target.port, 443);
}

/// A lookup attaches nothing to the calling thread's default context.
/// After lookups under a fresh context pushed on a test thread,
/// g_main_context_pending is false.
#[test]
fn lookups_attach_nothing_to_the_callers_thread_default_context() {
    let handle = std::thread::spawn(|| {
        // SAFETY: creating and pushing a private context for this thread.
        let context = unsafe { glib_sys::g_main_context_new() };
        unsafe { glib_sys::g_main_context_push_thread_default(context) };

        for _ in 0..16 {
            assert_eq!(
                SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
                None
            );
        }

        // SAFETY: context is valid and pushed on this thread.
        let pending = unsafe { glib_sys::g_main_context_pending(context) };
        unsafe {
            glib_sys::g_main_context_pop_thread_default(context);
            glib_sys::g_main_context_unref(context);
        }

        assert_eq!(
            pending,
            glib_sys::GFALSE,
            "expected no pending sources on caller context"
        );
    });

    handle.join().expect("thread join succeeded");
}

/// Concurrent lookups from several threads all answer correctly, with no
/// deadlock.
#[test]
fn concurrent_lookups_from_several_threads_all_answer_correctly() {
    let handles: Vec<_> = (0..8)
        .map(|i| {
            std::thread::spawn(move || {
                for _ in 0..8 {
                    assert_eq!(
                        SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
                        None
                    );
                }
                assert_eq!(
                    SystemResolver.lookup_srv(&format!("test-{i}\0.invalid")),
                    None
                );
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread join succeeded");
    }
}
