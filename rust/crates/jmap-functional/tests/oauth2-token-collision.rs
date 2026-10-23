// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Two JMAP accounts with the same `[Authentication] User` share one OAuth2
//! secret-store slot. `eos_generate_secret_uid` (evolution-data-server
//! 3.52.3, `e-oauth2-service.c:1086`) makes the slot key
//! `"OAuth2::<service name>[<user>]"`, with no host. Each in-tree
//! `EOAuth2Service` names one cloud, so the pair is unique there. JMAP has
//! many servers, so two accounts on two unrelated deployments with the same
//! address collide.
//!
//! ## Why two mock servers, not a live Fastmail/Stalwart pair
//!
//! The real scenario is a Stalwart test server and a Fastmail account with
//! the same address. This runner has no Fastmail credentials, and a test with
//! two real servers needs a GUI consent flow, but this VM has no display. The
//! defect is in the EDS key derivation, not in the server answers. Thus two
//! mock deployments reproduce the same bug deterministically, without live
//! infrastructure.
//!
//! ## The shape of the proof
//!
//! Account A gets a refresh token and connects to mock server A. Server A
//! issues a long-lived access token, and `eos_store_token_sync` stores it in
//! the shared slot. Then `functional-oauth2-token-collision-client` asks for a
//! token for account B, which has no consent. This is the same as the factory
//! of a new account. Two facts prove the collision. The token endpoint of
//! mock server B gets no request, because the lookup in the shared slot finds
//! a token that is not expired. Account B gets the exact access token that
//! mock server A issued for account A.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use serde_json::json;

/// The `[Authentication] User` of the two accounts. Per
/// `eos_generate_secret_uid`, this is the precondition for the collision.
const SECRET_USER: &str = "jmap-oauth2-collision";

/// The key from `jmap_config::oauth2_service::NAME` (`"JMAP"`) and
/// [`SECRET_USER`]. It contains no host.
const SECRET_UID: &str = "OAuth2::JMAP[jmap-oauth2-collision]";

/// The initial refresh token of account A. It has no `expires_after`, so
/// `eos_lookup_token_sync` gives `-1`. Thus
/// `e_source_get_oauth2_access_token_sync` refreshes on the first fetch of
/// account A (`e-oauth2-service.c:1893`, `TOKEN_VALIDITY_GAP_SECS` is 10),
/// with no timing window.
const SEED_REFRESH_TOKEN: &str = "seed-refresh-token-account-a";

/// The access token that mock server A issues for the refresh of account A.
/// Server B cannot make this string, so if account B gets it, it comes from
/// server A.
const SERVER_A_ACCESS_TOKEN: &str = "server-a-access-token";

/// Much longer than `TOKEN_VALIDITY_GAP_SECS` (10). Thus the store alone
/// answers the fetch of account B, with no refresh and no network call.
const TOKEN_LIFETIME_SECONDS: u64 = 3600;

/// An account keyfile with [`SECRET_USER`] and a `TokenEndpoint` for one
/// deployment. It uses `[Collection]`, not `[Calendar]`/`[Mail]`, because the
/// test opens only the registry, not a factory (as `oauth2-stale-proxy.rs`
/// does). It needs only the "JMAP" `EOAuth2Service` and the `[JMAP OAuth2]`
/// extension type from `module-jmap-backend.so`.
fn keyfile(display_name: &str, port: u16) -> String {
    Keyfile::new(
        SourceKind::Collection {
            contacts: false,
            calendars: false,
            mail: false,
        },
        port,
    )
    .display_name(display_name)
    .user(SECRET_USER)
    .auth_method("JMAP")
    .oauth2()
    .to_string()
}

/// The form bodies that the token endpoint of a mock deployment received,
/// in sequence. The handler closure writes them and the assertions read them.
type TokenRequestLog = Arc<Mutex<Vec<BTreeMap<String, String>>>>;

/// Starts a mock deployment that issues `issued_access_token` and records
/// each token request. In this test, only server A gets requests.
fn counted_oauth_server(
    issued_access_token: &'static str,
) -> (jmap_mock::MockServer, TokenRequestLog) {
    let token_requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&token_requests);
    let server = jmap_mock::MockServer::builder()
        .oauth_token(move |fields| {
            recorded
                .lock()
                .expect("token request log")
                .push(fields.clone());
            (
                200,
                json!({
                    "access_token": issued_access_token,
                    "token_type": "Bearer",
                    "expires_in": TOKEN_LIFETIME_SECONDS,
                    "refresh_token": "server-issued-refresh-token",
                }),
            )
        })
        .start();
    (server, token_requests)
}

#[test]
fn two_accounts_sharing_a_user_string_share_one_token_slot() {
    let client = required_path("JMAP_FUNCTIONAL_OAUTH2_TOKEN_COLLISION_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");
    let oauth2_services = required_path("JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE");

    // The handler of server B also records requests. The test asserts that
    // it gets none.
    let (server_a, requests_a) = counted_oauth_server(SERVER_A_ACCESS_TOKEN);
    let (server_b, requests_b) = counted_oauth_server("server-b-access-token");

    let port_of = |server: &jmap_mock::MockServer| -> u16 {
        server
            .origin()
            .rsplit_once(':')
            .expect("the mock's origin ends in a port")
            .1
            .parse()
            .expect("the mock's port is a number")
    };
    let port_a = port_of(&server_a);
    let port_b = port_of(&server_b);

    const ACCOUNT_A: &str = "jmap-functional-oauth2-collision-a";
    const ACCOUNT_B: &str = "jmap-functional-oauth2-collision-b";

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/oauth2-token-collision"
    ));
    session.write_source(ACCOUNT_A, &keyfile("JMAP collision account A", port_a));
    session.write_source(ACCOUNT_B, &keyfile("JMAP collision account B", port_b));
    session.stage_collection_backend(&module);
    // Keep this after the line above. Both write EDS_REGISTRY_MODULES, and
    // this call adds to the directory that the previous call created.
    session.stage_installed_registry_module(&oauth2_services);

    // Account A has a refresh token. Its first fetch is a refresh grant to
    // mock server A, which issues SERVER_A_ACCESS_TOKEN into the shared slot.
    let seed = json!({ "refresh_token": SEED_REFRESH_TOKEN }).to_string();
    let output_a = session.run(&client, &[ACCOUNT_A, SECRET_UID, &seed]);
    let stdout_a = String::from_utf8_lossy(&output_a.stdout).into_owned();
    let stderr_a = String::from_utf8_lossy(&output_a.stderr);
    let report_a =
        format!("--- account A stdout ---\n{stdout_a}--- account A stderr ---\n{stderr_a}");
    let seen_a = observations(&stdout_a);

    assert!(
        output_a.status.success(),
        "account A's client failed with {}\n{report_a}",
        output_a.status
    );
    assert_eq!(
        seen_a.get("fetched"),
        Some(&"1"),
        "account A could not fetch a token at all, before any collision is even \
         possible\n{report_a}"
    );
    assert_eq!(
        seen_a.get("access-token"),
        Some(&SERVER_A_ACCESS_TOKEN),
        "account A did not receive the token its own server minted\n{report_a}"
    );
    assert_eq!(
        requests_a.lock().expect("server A's request log").len(),
        1,
        "account A's own refresh did not reach server A's token endpoint exactly \
         once\n{report_a}"
    );

    // Account B has no token and no consent, as a new account. Its Host,
    // Port and TokenEndpoint all name server B.
    let output_b = session.run(&client, &[ACCOUNT_B, SECRET_UID, ""]);
    let stdout_b = String::from_utf8_lossy(&output_b.stdout).into_owned();
    let stderr_b = String::from_utf8_lossy(&output_b.stderr);
    let report_b =
        format!("--- account B stdout ---\n{stdout_b}--- account B stderr ---\n{stderr_b}");
    let seen_b = observations(&stdout_b);

    assert!(
        output_b.status.success(),
        "account B's client failed with {}\n{report_b}",
        output_b.status
    );
    assert_eq!(
        seen_b.get("fetched"),
        Some(&"1"),
        "account B, never consented to at all, could not fetch a token, which \
         would mean this reproduction failed to trigger the collision rather \
         than that the underlying bug is gone\n{report_b}"
    );

    // Two independent proofs of the collision.
    assert_eq!(
        requests_b.lock().expect("server B's request log").len(),
        0,
        "server B's own token endpoint was asked something — account B's fetch \
         should have been answered entirely out of the shared secret-store slot, \
         never by talking to its own server at all\n{report_b}"
    );
    assert_eq!(
        seen_b.get("access-token"),
        Some(&SERVER_A_ACCESS_TOKEN),
        "account B did not receive account A's own access token back: the \
         reproduction did not trigger the (service, user) secret-store \
         collision\n{report_b}"
    );
}
