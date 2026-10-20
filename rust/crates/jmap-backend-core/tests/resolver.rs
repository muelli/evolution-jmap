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

/// Bytes that this process allocated and did not free. This uses
/// `mallinfo2().uordblks`, not RSS, for the reason in
/// jmap-backend-cal/tests/references.rs: a leaked completion source is ~519
/// bytes, which is not visible at page granularity.
fn allocated_bytes() -> u64 {
    // SAFETY: no preconditions; returns a plain struct of counters.
    unsafe { libc::mallinfo2() }.uordblks as u64
}

/// Regression test for glib#4041. The sync `g_resolver_lookup_service` puts a
/// `GTask` plus its completion-idle `GSource` (~519 bytes per call) on the
/// calling thread's thread-default main context. GLib frees them only when
/// that context iterates, and an EDS worker thread never iterates it. GLib
/// answered "works as designed; iterate the context". Thus the lookup now runs
/// under a private, drained context. This test fails if that regresses.
///
/// Windowed retention as in `references.rs`: pass when the growth of any
/// window is noise. Without the fix, each window retains ~512 x 519 B, eight
/// times the floor. The resolver has a 30 s timeout source per lookup
/// (self-draining, on GLib's own worker context, which this test cannot
/// control). The test disables it first with timeout 0. Thus timeouts that are
/// pending but not leaked cannot look like retention here.
#[test]
fn repeated_lookups_retain_nothing_on_the_callers_context() {
    // If the fix's Drop releases a reference too many times, GLib logs a
    // critical. Make criticals abort the run so that they are not missed.
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: no preconditions; the previous mask is returned and ignored.
        unsafe {
            glib_sys::g_log_set_always_fatal(
                glib_sys::G_LOG_LEVEL_CRITICAL | glib_sys::G_LOG_LEVEL_WARNING,
            )
        };
    });
    // Declared here because the workspace's gio-sys is older than this GLib
    // 2.78 symbol. Every supported runtime (Ubuntu 24.04's 2.80 and later) has
    // it.
    unsafe extern "C" {
        fn g_resolver_set_timeout(resolver: *mut gio_sys::GResolver, timeout_ms: std::ffi::c_uint);
    }
    // SAFETY: get_default returns a strong reference, and the unref releases
    // it. Timeout 0 disables the per-lookup timeout source.
    unsafe {
        let resolver = gio_sys::g_resolver_get_default();
        g_resolver_set_timeout(resolver, 0);
        gobject_sys::g_object_unref(resolver.cast());
    }

    const LOOKUPS: u64 = 512;
    const WINDOWS: usize = 6;
    const NOISE_FLOOR: u64 = 32 * 1024;
    let mut retained = Vec::with_capacity(WINDOWS);
    for _ in 0..WINDOWS {
        let before = allocated_bytes();
        for _ in 0..LOOKUPS {
            assert_eq!(
                SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
                None
            );
        }
        let growth = allocated_bytes().saturating_sub(before);
        if growth <= NOISE_FLOOR {
            return;
        }
        retained.push(growth);
    }
    panic!(
        "no window of {LOOKUPS} lookups retained less than the {NOISE_FLOOR}-byte noise floor; \
         bytes retained per window: {retained:?}. glib#4041's completion sources are ~519 B per \
         call, and only a drained private context keeps a window under the floor."
    );
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

/// The number of open file descriptors does not grow across lookups.
/// This prevents GWakeup fd exhaustion from contexts that never iterate.
#[test]
fn open_file_descriptors_do_not_grow_across_repeated_lookups() {
    fn count_open_fds() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .expect("reading /proc/self/fd succeeds")
            .count()
    }

    // Do one lookup first to start the resolver thread and do the one-time
    // process allocations.
    assert_eq!(
        SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
        None
    );

    let fds_before = count_open_fds();

    for _ in 0..256 {
        assert_eq!(
            SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
            None
        );
    }

    let fds_after = count_open_fds();

    assert!(
        fds_after <= fds_before,
        "fd count grew from {fds_before} to {fds_after} across 256 lookups"
    );
}
