// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `config-lookup.rs`'s real-Stalwart counterpart: a real `EConfigLookup`
//! running `JmapConfigLookup` against real Stalwart's own OAuth 2.0
//! discovery endpoint, not the mock's simplified fixture.
//!
//! ## Why this cannot assert a positive, complete result
//!
//! RFC 8414 §3.3 requires the fetched metadata document's `issuer` to equal,
//! byte for byte, the host discovery was asked for. Real Stalwart's issuer is
//! baked in at first-boot time from the one-time Bootstrap singleton's
//! `serverHostname` ("https://mail.example.internal" on this runner's test
//! server, confirmed live) and nothing in the post-bootstrap admin API can
//! change it: the `Bootstrap` singleton itself is gone once the server leaves
//! bootstrap mode. So any address this runner can actually reach Stalwart on
//! — the loopback proxy's own origin, same as every other live-stalwart test
//! uses — names a different issuer than the one the document states, and
//! `oauth::discover`'s own RFC 8414 §3.3 check rejects it.
//! `jmap-config/tests/live_server_discover_and_register.rs` solved the same
//! mismatch one layer down with a transport that rewrites just the origin
//! while leaving the checked issuer string untouched, but
//! `JmapConfigLookup::run()` constructs its own `UreqTransport` directly with
//! no such seam, and adding one would be production-code scope creep for a
//! coverage-only item.
//!
//! What this test asserts instead is still genuinely new, real-server
//! coverage: that `run()`'s *failure* path — reached here by a real,
//! non-synthetic RFC 8414 rejection of real Stalwart's own, fuller metadata
//! document (it carries `device_authorization_endpoint`,
//! `introspection_endpoint`, `token_endpoint_auth_methods_supported` and
//! `authorization_response_iss_parameter_supported`, none of which the mock's
//! fixture does) — actually reaches a real `EConfigLookup`'s own
//! `"worker-finished"` signal with a real `GError`, and adds no result. No
//! existing test drives this path at all: `config-lookup.rs` only ever
//! exercises the mock's success path, and `jmap-config`'s own unit tests of
//! `Error::to_gerror` never go through a real `EConfigLookup`.
//!
//! Needs no throwaway account: RFC 8414 discovery and RFC 7591 registration
//! are both unauthenticated by design, so only `$STALWART_URL` is required.
//!
//! ## Running it
//!
//! ```console
//! $ source harness/live-server/live-server-env.sh
//! $ cargo test -p jmap-functional --test live-stalwart-config-lookup -- --ignored
//! ```
//!
//! Skipped, not failed, when `STALWART_URL` is unset.

use std::env;

use jmap_functional::{Session, observations, required_path, spawn_loopback_proxy};

const EMAIL_ADDRESS: &str = "agent-fnconfiglookup@example.com";

#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_real_config_lookup_rejects_real_stalwarts_mismatched_issuer() {
    let Some(origin) = env::var("STALWART_URL").ok() else {
        eprintln!("STALWART_URL not set; skipping the real-server config-lookup leg");
        return;
    };
    let client = required_path("JMAP_FUNCTIONAL_CONFIG_LOOKUP_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CONFIG_LOOKUP_MODULE");

    let authority = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let port = spawn_loopback_proxy(authority.to_owned());
    let proxy_origin = format!("http://127.0.0.1:{port}");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/live-stalwart-config-lookup"
    ));
    let module_dir = session.stage_config_lookup_module(&module);

    let output = session.run(
        &client,
        &[
            module_dir
                .to_str()
                .expect("the session's module directory is UTF-8"),
            EMAIL_ADDRESS,
            &proxy_origin,
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("worker-finished-seen"),
        Some(&"1"),
        "the worker never reached EConfigLookup's own \"worker-finished\" \
         signal\n{report}"
    );

    let message = seen.get("worker-error-message").unwrap_or_else(|| {
        panic!(
            "real Stalwart's issuer should not match the proxy's own origin, \
             so discovery should have failed\n{report}"
        )
    });
    assert!(
        message.contains("RFC 8414"),
        "the failure should be the RFC 8414 §3.3 issuer mismatch, not \
         something else: {message:?}\n{report}"
    );

    assert_eq!(
        seen.get("result-count"),
        Some(&"0"),
        "a rejected discovery must not add a result\n{report}"
    );
}
