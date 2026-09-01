// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Two JMAP accounts that authenticate as the
//! same `[Authentication] User` share one OAuth2 secret-store slot, because
//! `eos_generate_secret_uid` (evolution-data-server 3.52.3,
//! `e-oauth2-service.c:1086`, read rather than assumed) derives that slot's
//! key as `"OAuth2::<service name>[<user>]"` — no host. Every in-tree
//! `EOAuth2Service` names exactly one cloud, so the pair is unique there;
//! JMAP is multi-server, so an account at one deployment and an account at a
//! completely unrelated one, both authenticating as the same address,
//! collide.
//!
//! ## Why two mock servers, not a live Fastmail/Stalwart pair
//!
//! Item 41's own text describes reproducing this against the real Stalwart
//! test server and a Fastmail account sharing an address. Neither is
//! reachable headlessly from here: this runner holds no Fastmail
//! credentials (the operator's real account is the only one that exists),
//! and a literal two-real-server repro would need a GUI consent flow this
//! VM has no display for. The defect itself is not about what either server
//! answers — it is entirely in EDS's own key derivation, independent of
//! which two servers are involved — so two independently configured mock
//! deployments, standing in for "Fastmail" and "a self-hosted Stalwart" the
//! way every other functional test here stands one mock in for "a real JMAP
//! server", reproduce the identical bug deterministically and are
//! reproducible by anyone who checks this repository out, live
//! infrastructure or not.
//!
//! ## The shape of the proof
//!
//! Account A seeds a refresh token and connects for real against mock
//! server A, which mints a fresh, long-lived access token that
//! `eos_store_token_sync` files under the shared slot. Account B then asks
//! for a token of its own, having consented to nothing — and
//! `functional-oauth2-token-collision-client` is run for it exactly as a
//! freshly added account's factory would run. Two facts nail the collision
//! down at once: mock server B's token endpoint is asked nothing at all
//! (account B never even attempts a refresh of its own, because the lookup
//! under the shared slot already finds an unexpired token), and the token
//! account B is handed is not merely *a* valid-looking string but the
//! byte-for-byte access token mock server A minted for account A.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jmap_functional::{Session, observations, required_path};
use serde_json::json;

/// `[Authentication] User`, shared by both accounts — the whole precondition
/// for the collision, per `eos_generate_secret_uid`.
const SECRET_USER: &str = "jmap-oauth2-collision";

/// `jmap_config::oauth2_service::NAME` is `"JMAP"`; this is the key that
/// name and [`SECRET_USER`] produce, host-blind by construction.
const SECRET_UID: &str = "OAuth2::JMAP[jmap-oauth2-collision]";

/// What account A is seeded as already holding — a refresh token with no
/// `expires_after`, which makes `eos_lookup_token_sync` report `-1` and so
/// forces `e_source_get_oauth2_access_token_sync` to refresh on account A's
/// very first fetch (`e-oauth2-service.c:1893`, `TOKEN_VALIDITY_GAP_SECS`
/// is 10) — deterministic, no timing window.
const SEED_REFRESH_TOKEN: &str = "seed-refresh-token-account-a";

/// What mock server A's token endpoint mints for account A's refresh. Chosen
/// distinct from anything server B could ever produce, so if account B's
/// fetch reports this string back there is only one place it could have
/// come from.
const SERVER_A_ACCESS_TOKEN: &str = "server-a-access-token";

/// Long enough that the token is nowhere near
/// `TOKEN_VALIDITY_GAP_SECS` (10) by the time account B looks the shared
/// slot up moments later, so account B's fetch is answered out of the store
/// alone — no refresh, no network call to either server.
const TOKEN_LIFETIME_SECONDS: u64 = 3600;

/// An account keyfile naming [`SECRET_USER`] and a `TokenEndpoint` unique to
/// this one deployment. `[Collection]` rather than `[Calendar]`/`[Mail]`:
/// this test never opens a factory, only the registry (the same shape
/// `oauth2-stale-proxy.rs` uses), so nothing beyond `module-jmap-backend.so`
/// registering the "JMAP" `EOAuth2Service` and `[JMAP OAuth2]`'s extension
/// type is needed.
fn keyfile(display_name: &str, port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName={display_name}\n\
         Enabled=true\n\
         \n\
         [Collection]\n\
         BackendName=jmap\n\
         ContactsEnabled=false\n\
         CalendarEnabled=false\n\
         MailEnabled=false\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         User={SECRET_USER}\n\
         Method=JMAP\n\
         \n\
         [JMAP OAuth2]\n\
         ClientId=jmap-functional-client\n\
         AuthorizationEndpoint=http://127.0.0.1:{port}/oauth/authorize\n\
         TokenEndpoint=http://127.0.0.1:{port}/oauth/token\n\
         RedirectUri=org.gnome.evolution.jmap:/redirect\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// Every form body a mock deployment's token endpoint was asked with, in
/// order — shared between the handler closure that appends to it and the
/// assertions that read it back afterwards.
type TokenRequestLog = Arc<Mutex<Vec<BTreeMap<String, String>>>>;

/// A mock deployment's own count of how many times its token endpoint was
/// asked, plus what it hands out. `issued_access_token` is only ever
/// consulted by server A's handler in this test — server B's is asked
/// nothing at all, which is exactly the fact under test.
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

    // Server B's handler would report a request too, if account B's fetch
    // ever reached it — the assertion below is that it never does.
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
    // Must come after the line above: both write EDS_REGISTRY_MODULES, and
    // this one adds to the directory that one created.
    session.stage_installed_registry_module(&oauth2_services);

    // Account A: seeded with a refresh token, so its first fetch is a real
    // refresh grant against mock server A, minting SERVER_A_ACCESS_TOKEN
    // and filing it under the shared slot.
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

    // Account B: never seeded, never consented — an ordinary "just added
    // this account" fetch, run against a source whose Host, Port and
    // TokenEndpoint all name server B and nothing about server A.
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
        "account B, never consented to at all, could not fetch a token — which \
         would mean this reproduction failed to trigger the collision rather \
         than that item 41 is wrong\n{report_b}"
    );

    // The collision itself, from two independent directions.
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
        "account B did not receive account A's own access token back — the \
         reproduction did not trigger the (service, user) secret-store \
         collision item 41 describes\n{report_b}"
    );
}
