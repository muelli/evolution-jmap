// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The real `_jmap._tcp` SRV resolver, against the system's real DNS.
//
// `jmap-client` ships only the seam and a `NoSrvResolver` that never finds
// anything, so until something implements `Resolver` for real, a plain
// email+password account still fetches `https://<email domain>/.well-known/
// jmap` and still 404s for a provider that publishes JMAP via SRV (Fastmail).
// These tests cover the half of that which is deterministic without a
// network: that a domain with no record answers "none" — the answer that
// preserves the bare-domain fallback and therefore every self-hosted
// deployment — and that neither the FFI call nor the input conversion can
// panic or misbehave on repetition.
//
// The success path needs a domain that really publishes a `_jmap._tcp` record,
// so its test is `#[ignore]`d rather than pretended to be hermetic; run it
// with `cargo test -p jmap-backend-core --test resolver -- --ignored`.

use jmap_backend_core::resolver::SystemResolver;
use jmap_client::resolver::Resolver;

/// RFC 6761 reserves `.invalid` precisely so that a name under it never
/// resolves, which is the deterministic half of this: whether this runner has
/// DNS egress or not, the lookup fails and the answer must be "no record", not
/// an error the caller has to handle — a failed SRV lookup means "try the bare
/// domain", which is what every deployment that answers at its own domain
/// (Stalwart, self-hosted, and the mock) relies on.
#[test]
fn a_domain_with_no_srv_record_reports_none_so_the_bare_domain_is_tried() {
    assert_eq!(
        SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
        None
    );
}

/// A domain string reaches C as a NUL-terminated string, so one containing an
/// interior NUL cannot be passed on. It comes from an `ESource` keyfile a user
/// can hand-write, so the conversion failing must read as "no record" like any
/// other unresolvable name rather than panicking inside a vfunc.
#[test]
fn a_domain_with_an_interior_nul_reports_none_rather_than_panicking() {
    assert_eq!(SystemResolver.lookup_srv("exa\0mple.com"), None);
    assert_eq!(SystemResolver.lookup_srv("\0"), None);
}

/// Every lookup takes a `GResolver` reference and a `GSrvTarget` list from
/// GLib and has to hand both back. Getting that wrong is invisible in a single
/// call — it shows up as a leak or a crash only under real use, where a backend
/// reconnects for the lifetime of an Evolution session — so the failing path,
/// the one that returns no list at all and an error instead, is walked enough
/// times that an unbalanced reference or free has somewhere to show.
#[test]
fn repeated_lookups_are_stable() {
    for _ in 0..64 {
        assert_eq!(
            SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
            None
        );
    }
}

/// The success path, against the provider whose 404 started this. Not
/// hermetic — it needs DNS egress and it asserts a third party's live DNS
/// record — so it is ignored by default and run by hand. What it proves is the
/// part no fake can: that the `GSrvTarget` list GLib returns is walked and read
/// correctly, and that the host is normalised into something
/// `ClientBuilder::connect_domain` can build a URL from (no trailing dot).
#[test]
#[ignore = "needs DNS egress and asserts a third party's live SRV record"]
fn fastmail_publishes_its_jmap_host_via_srv() {
    let target = SystemResolver
        .lookup_srv("fastmail.com")
        .expect("_jmap._tcp.fastmail.com publishes an SRV record");

    assert_eq!(target.host, "api.fastmail.com");
    assert_eq!(target.port, 443);
}

/// Bytes this process has allocated and not returned. `mallinfo2().uordblks`
/// rather than RSS for jmap-backend-cal/tests/references.rs's reason: a leaked
/// completion source is ~519 bytes, invisible at page granularity.
fn allocated_bytes() -> u64 {
    // SAFETY: no preconditions; returns a plain struct of counters.
    unsafe { libc::mallinfo2() }.uordblks as u64
}

/// glib#4041, pinned: the sync `g_resolver_lookup_service` parks a `GTask`
/// plus its completion-idle `GSource` (~519 bytes per call) on the calling
/// thread's thread-default main context, reclaimed only once that context is
/// iterated, which an EDS worker thread never does. GLib answered "works as
/// designed; iterate the context", so the lookup now runs under a private,
/// drained context, and this test is what fails if that regresses.
///
/// Windowed retention as in `references.rs`: pass when any window's growth is
/// noise. Unfixed, every window retains ~512 x 519 B, eight times the floor.
/// The resolver's per-lookup 30 s timeout source (self-draining, on GLib's own
/// worker context, outside our reach) is disabled up front with timeout 0 so
/// pending-but-not-leaked timeouts cannot masquerade as retention here.
#[test]
fn repeated_lookups_retain_nothing_on_the_callers_context() {
    // A reference over-released by the fix's Drop would surface as a GLib
    // critical; make those abort the run rather than scroll past.
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: no preconditions; the previous mask is returned and ignored.
        unsafe {
            glib_sys::g_log_set_always_fatal(
                glib_sys::G_LOG_LEVEL_CRITICAL | glib_sys::G_LOG_LEVEL_WARNING,
            )
        };
    });
    // Declared locally: the workspace's gio-sys predates this GLib 2.78
    // symbol, but every supported runtime (Ubuntu 24.04's 2.80 up) has it.
    unsafe extern "C" {
        fn g_resolver_set_timeout(resolver: *mut gio_sys::GResolver, timeout_ms: std::ffi::c_uint);
    }
    // SAFETY: get_default returns a strong reference, balanced by the unref;
    // timeout 0 disables the per-lookup timeout source.
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
