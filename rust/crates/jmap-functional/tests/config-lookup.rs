// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, config lookup: a real `EConfigLookup` running `JmapConfigLookup`
//! — the `EConfigLookupWorker` behind the account assistant's "Look Up Account
//! Details" step — against the mock's OAuth 2.0 discovery endpoints
//! (RFC 8414/7591).
//!
//! Unlike every other functional test in this crate, nothing here is scanned
//! by a daemon: `JmapConfigLookup` registers itself the moment a real
//! `EConfigLookup` is constructed in the *same* process (see
//! `jmap_config::module`'s doc comment for why), so
//! `tests/functional/config-lookup-client.c` loads
//! `module-jmap-configuration.so` itself rather than pointing a factory at
//! it, and there is no `.source` keyfile — the whole point of a lookup is
//! that no account exists yet.
//!
//! `run()`'s network half (`jmap_config::oauth2_setup::discover_and_register`)
//! is exercised elsewhere against the mock (`jmap-config`'s own
//! `tests/oauth2_setup.rs`); what only this harness can check is that a
//! *real* `EConfigLookup` actually dispatches into it and that the result it
//! adds configures a scratch `ESource`'s `[Collection]`/`[Authentication]`/
//! `[Security]` extensions the way the account assistant's own
//! `configure_source` call would. The 307th session's log
//! (`docs/NIGHT-LOG.md`) hand-drove exactly this once with a throwaway
//! client; this is that spike made permanent.

use jmap_functional::{Session, observations, required_path};
use jmap_mock::MockServer;
use serde_json::json;

/// The email address handed to the lookup. Its domain is never reached:
/// `servers` (the mock's own origin, below) names the host explicitly and
/// wins over the domain fallback, the same precedence
/// `config_lookup::probe_host`'s own unit tests pin.
const EMAIL_ADDRESS: &str = "vera@example.com";

/// The `client_id` the mock's registration endpoint hands back, checked
/// against nothing here — `jmap-config`'s own `oauth2_setup.rs` already pins
/// that this value reaches `Config::client_id` unchanged. This test's job is
/// the layer above that: whether a real `EConfigLookup` result carries a
/// complete account at all.
const CLIENT_ID: &str = "config-lookup-functional-test-client";

fn metadata(origin: &str) -> serde_json::Value {
    json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/authorize"),
        "token_endpoint": format!("{origin}/oauth/token"),
        "registration_endpoint": format!("{origin}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
    })
}

#[test]
fn a_real_config_lookup_discovers_and_configures_an_oauth2_jmap_account() {
    let client = required_path("JMAP_FUNCTIONAL_CONFIG_LOOKUP_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CONFIG_LOOKUP_MODULE");

    let server = MockServer::builder()
        .oauth_authorization_server(metadata)
        .oauth_client_registration(|_request| (201, json!({"client_id": CLIENT_ID})))
        .start();

    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/config-lookup"));
    let module_dir = session.stage_config_lookup_module(&module);

    // `server.origin()` is `http://127.0.0.1:<port>` — exactly the
    // `scheme://host:port` shape `config_lookup::parse_target` reads a
    // `servers` override in, and the one thing that lets a plaintext,
    // non-default-port test deployment be named at all (the 307th session's
    // fix; see `docs/NIGHT-LOG.md`).
    let output = session.run(
        &client,
        &[
            module_dir
                .to_str()
                .expect("the session's module directory is UTF-8"),
            EMAIL_ADDRESS,
            server.origin(),
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
        seen.get("result-count"),
        Some(&"1"),
        "the lookup did not add exactly one result\n{report}"
    );
    assert_eq!(
        seen.get("protocol"),
        Some(&"jmap"),
        "the result named the wrong protocol\n{report}"
    );
    assert_eq!(
        seen.get("is-complete"),
        Some(&"1"),
        "the account assistant would not offer to finish setup from this \
         result\n{report}"
    );

    // What `e_config_lookup_result_configure_source` wrote onto a scratch
    // `ESource` — the same call the assistant makes when the user picks this
    // result, and the only way to read a "simple" result's values back (see
    // `config-lookup-client.c`'s own comment on why).
    assert_eq!(
        seen.get("collection-backend-name"),
        Some(&"jmap"),
        "the result did not configure a JMAP collection backend\n{report}"
    );
    assert_eq!(
        seen.get("collection-identity"),
        Some(&EMAIL_ADDRESS),
        "the result lost the identity address\n{report}"
    );
    assert_eq!(
        seen.get("authentication-host"),
        Some(&"127.0.0.1"),
        "the result did not name the mock's own host\n{report}"
    );
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");
    assert_eq!(
        seen.get("authentication-port"),
        Some(&port.to_string().as_str()),
        "the result did not name the mock's own port\n{report}"
    );
    assert_eq!(
        seen.get("authentication-user"),
        Some(&EMAIL_ADDRESS),
        "the result lost the username\n{report}"
    );
    assert_eq!(
        seen.get("authentication-method"),
        Some(&"JMAP"),
        "the result did not select this crate's own EOAuth2Service\n{report}"
    );
    assert_eq!(
        seen.get("security-method"),
        Some(&"none"),
        "the result claimed TLS against a plaintext mock\n{report}"
    );
}
