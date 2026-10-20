// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Config lookup: a real `EConfigLookup` runs `JmapConfigLookup` against the
//! OAuth 2.0 discovery endpoints of the mock (RFC 8414/7591).
//! `JmapConfigLookup` is the `EConfigLookupWorker` for the "Look Up Account
//! Details" step of the account assistant.
//!
//! No daemon scans a module here. `JmapConfigLookup` registers itself when a
//! real `EConfigLookup` is constructed in the same process (see the doc
//! comment of `jmap_config::module`). Thus
//! `tests/functional/config-lookup-client.c` loads
//! `module-jmap-configuration.so` itself. There is no `.source` keyfile,
//! because a lookup occurs before an account exists.
//!
//! `jmap-config`'s `tests/oauth2_setup.rs` tests the network part of `run()`
//! (`jmap_config::oauth2_setup::discover_and_register`) against the mock.
//! This test checks the part that other tests cannot check: a real
//! `EConfigLookup` dispatches into `run()`, and the result configures the
//! `[Collection]`/`[Authentication]`/`[Security]` extensions of a scratch
//! `ESource` the same way as the `configure_source` call of the account
//! assistant.

use jmap_functional::{Session, observations, required_path};
use jmap_mock::MockServer;
use serde_json::json;

/// The email address for the lookup. The test never connects to its domain:
/// `servers` (the origin of the mock) names the host and has priority over
/// the domain fallback. The unit tests of `config_lookup::probe_host` check
/// this priority.
const EMAIL_ADDRESS: &str = "vera@example.com";

/// The `client_id` that the registration endpoint of the mock returns. This
/// test does not check it: `jmap-config`'s `oauth2_setup.rs` checks that it
/// gets to `Config::client_id`. This test checks only that the result of a
/// real `EConfigLookup` contains a complete account.
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

    // `server.origin()` is `http://127.0.0.1:<port>`. This is the
    // `scheme://host:port` form that `config_lookup::parse_target` reads from
    // a `servers` override. Only this form can name a plaintext test server
    // on a non-default port.
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

    // The values that `e_config_lookup_result_configure_source` wrote to a
    // scratch `ESource`. The assistant makes the same call when the user
    // selects this result. It is the only way to read back the values of a
    // "simple" result (see the comment in `config-lookup-client.c`).
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
