// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The real `_jmap._tcp` SRV resolver, against the system's real DNS.
//
// `jmap-client` ships only the seam and a `NoSrvResolver` that never finds
// anything, so until something implements `Resolver` for real, a plain
// email+password account still fetches `https://<email domain>/.well-known/
// jmap` and still 404s for a provider that publishes JMAP via SRV (Fastmail —
// see `docs/NIGHT-LOG.md`, "JMAP SRV autodiscovery"). These tests cover the
// half of that which is deterministic without a network: that a domain with no
// record answers "none" — the answer that preserves the bare-domain fallback
// and therefore every self-hosted deployment — and that neither the FFI call
// nor the input conversion can panic or misbehave on repetition.
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
