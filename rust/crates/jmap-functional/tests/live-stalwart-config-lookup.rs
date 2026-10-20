// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The real-Stalwart version of `config-lookup.rs`: a real `EConfigLookup`
//! runs `JmapConfigLookup` against the OAuth 2.0 discovery endpoint of real
//! Stalwart, not against the simplified fixture of the mock.
//!
//! ## Why this test cannot assert a complete result
//!
//! RFC 8414 §3.3 requires that the `issuer` of the metadata document is
//! byte-for-byte equal to the host that discovery asked for. Real Stalwart
//! sets its issuer at first boot from the `serverHostname` of the one-time
//! `Bootstrap` singleton ("https://mail.example.internal" on the test server
//! of this runner). The post-bootstrap admin API cannot change it, because
//! the `Bootstrap` singleton does not exist after bootstrap mode.
//!
//! Thus each address at which this runner can get to Stalwart (the origin of
//! the loopback proxy, as in all live-stalwart tests) is different from the
//! issuer in the document. The RFC 8414 §3.3 check in `oauth::discover`
//! rejects it.
//!
//! `jmap-config/tests/live_server_discover_and_register.rs` solves the same
//! mismatch one layer lower: its transport changes only the origin and keeps
//! the issuer string. But `JmapConfigLookup::run()` constructs its own
//! `UreqTransport` and has no such seam. To add one would change production
//! code only for test coverage.
//!
//! Thus this test asserts the failure path of `run()`. A real RFC 8414
//! rejection of the metadata document of real Stalwart causes the failure.
//! That document is more complete than the fixture of the mock: it has
//! `device_authorization_endpoint`, `introspection_endpoint`,
//! `token_endpoint_auth_methods_supported` and
//! `authorization_response_iss_parameter_supported`. The test asserts that
//! the failure gets to the `"worker-finished"` signal of a real
//! `EConfigLookup` with a real `GError`, and that no result is added.
//! `config-lookup.rs` tests only the success path of the mock, and the unit
//! tests of `Error::to_gerror` in `jmap-config` do not use a real
//! `EConfigLookup`.
//!
//! This test needs no throwaway account. RFC 8414 discovery and RFC 7591
//! registration need no authentication, thus only `$STALWART_URL` is
//! necessary.
//!
//! ## How to run it
//!
//! Set `STALWART_URL`, then run:
//!
//! ```console
//! $ cargo test -p jmap-functional --test live-stalwart-config-lookup -- --ignored
//! ```
//!
//! When `STALWART_URL` is not set, the test skips and does not fail.

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
