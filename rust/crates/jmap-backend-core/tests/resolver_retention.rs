// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This test changes the timeout of the process-wide default `GResolver`. A
// lookup on another thread reads that timeout, which is a data race. Thus this
// test has its own binary, and no other test can run a lookup at the same time.

use jmap_backend_core::resolver::SystemResolver;
use jmap_client::resolver::Resolver;

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
