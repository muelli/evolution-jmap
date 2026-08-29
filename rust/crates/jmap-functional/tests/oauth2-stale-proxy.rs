// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1: Roadmap item 22 headless reproduction test.
//!
//! "A stale client OAuth2Support proxy in the registry turns every token fetch
//! into a consent window."
//!
//! Mechanism:
//! 1. An Evolution shell instance connects to the session bus, acquires a unique
//!    bus name (e.g. `:1.X`), and exports an `EOAuth2Support` D-Bus object.
//! 2. When that shell instance is terminated (`kill -TERM`), crashes, or is
//!    replaced by single-instance handoff, its unique bus name vanishes from D-Bus.
//! 3. The registry / proxy layer retains the stale proxy to `:1.X` instead of
//!    falling back to its own built-in `EOAuth2Support` (`ESourceRegistryServer` /
//!    `EOAuth2Services`) or unbinding the dead proxy.
//! 4. Every subsequent silent token fetch directed to the dead proxy fails with
//!    `G_DBUS_ERROR_SERVICE_UNKNOWN` / "The name :1.X was not provided by any
//!    .service files" (`NAME_HAS_NO_OWNER`).
//! 5. When a new shell (Shell 2) starts with a new unique bus name (`:1.Y`), the
//!    existing stale proxy continues to point to `:1.X` and fails.
//!
//! This deterministic functional test exercises and pins this sequence.

use jmap_functional::{Session, observations, required_path};

fn keyfile() -> String {
    "[Data Source]\n\
     DisplayName=JMAP OAuth2 reproduction test account\n\
     Enabled=true\n\
     \n\
     [Collection]\n\
     BackendName=jmap\n\
     ContactsEnabled=true\n\
     CalendarEnabled=true\n\
     MailEnabled=false\n\
     \n\
     [Authentication]\n\
     Host=127.0.0.1\n\
     Port=1\n\
     Method=OAuth2\n\
     \n\
     [Security]\n\
     Method=none\n"
        .to_string()
}

#[test]
fn stale_oauth2_support_proxy_fails_with_service_unknown_after_shell_termination() {
    let client = required_path("JMAP_FUNCTIONAL_OAUTH2_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    const ACCOUNT_UID: &str = "jmap-functional-oauth2-repro";
    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/oauth2-stale-proxy-repro"
    ));
    session.write_source(ACCOUNT_UID, &keyfile());
    session.stage_collection_backend(&module);

    let output = session.run(&client, &[ACCOUNT_UID]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    // Step 1: Initial token fetch from Shell 1 succeeds.
    assert_eq!(
        seen.get("initial-token-success"),
        Some(&"1"),
        "initial token fetch via Shell 1 failed\n{report}"
    );
    assert_eq!(
        seen.get("initial-token"),
        Some(&"mock-token-shell-1"),
        "initial token value mismatch\n{report}"
    );

    // Step 2: Shell 1 is terminated via SIGTERM and its unique name disappears.
    assert_eq!(
        seen.get("shell-1-killed"),
        Some(&"1"),
        "Shell 1 was not confirmed killed\n{report}"
    );

    // Step 3: Token fetch via the stale proxy pointing to the dead unique name
    // fails with G_DBUS_ERROR_SERVICE_UNKNOWN / "not provided by any .service files".
    assert_eq!(
        seen.get("token-after-kill-success"),
        Some(&"0"),
        "expected token fetch via stale proxy to fail after shell termination\n{report}"
    );
    let error_msg = seen
        .get("token-after-kill-error-message")
        .copied()
        .unwrap_or_default();
    assert!(
        error_msg.contains("not provided by any .service files")
            || error_msg.contains("SERVICE_UNKNOWN")
            || error_msg.contains("NameHasNoOwner"),
        "error message did not contain expected D-Bus service unknown / name has no owner indication: '{error_msg}'\n{report}"
    );

    // Step 4: After starting Shell 2 with a new unique name, the stale proxy
    // still fails because it was never rebound or cleared.
    assert_eq!(
        seen.get("stale-proxy-still-fails"),
        Some(&"1"),
        "expected stale proxy to remain broken after new shell registered\n{report}"
    );
}
