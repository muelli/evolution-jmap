// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The calendar part of the stale-access-token acceptance test, without a
//! display. This backend refreshes through an `ESource`, not through a
//! `CamelSession`.
//!
//! The mail test (`mail-stale-token.rs`) overrides
//! `get_oauth2_access_token_sync` on a test `CamelSession`, because
//! `jmap_mail::oauth2::uses_oauth2` reads the Camel `auth-mechanism` field,
//! and the generic `[Authentication] Method=OAuth2` satisfies it. The
//! calendar backend uses `jmap_backend_core::oauth2::source_uses_oauth2`,
//! and gets its token from the registry. The account uses `Method=JMAP`,
//! which `e_oauth2_services_is_oauth2_alias` matches to the `EOAuth2Service`
//! of this project. For that method, the EDS `module-oauth2-services.so`
//! exports `Source.OAuth2Support`. Thus the refresh is real: it runs in the
//! registry, through our `EOAuth2Service`, against a real token endpoint.
//! No part of the chain is a substitute.
//!
//! ## How the refresh occurs without a wall clock
//!
//! The refresh does not depend on time. `e_oauth2_service_get_access_token_sync`
//! refreshes when the stored token has `expires_in <= TOKEN_VALIDITY_GAP_SECS`
//! (**10**, `e-oauth2-service.c:47`). `eos_lookup_token_sync` calculates
//! `expires_in` from the stored `expires_after`. If `expires_after` is
//! absent, the value is `-1`. Thus a seeded secret with only a
//! `refresh_token` sends *each* fetch to the token endpoint, and
//! [`TOKEN_LIFETIME_SECONDS`] keeps this true for the tokens that the
//! endpoint issues. The mock alone decides which access token is valid, as
//! the token file of `mail-stale-token.rs` does for mail.
//!
//! ## What each test examines
//!
//! Both tests do the same steps: connect, create an event, change the bearer
//! token that the server accepts, then create a second event. They differ
//! only in one thing: whether the token endpoint issues the new token.
//!
//! The important assertions are counts from the server side. In the positive
//! case, a backend that closes the connection and asks EDS for credentials
//! also creates the event, but the counts show the difference.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use jmap_functional::{Session, observations, required_path};
use serde_json::json;

/// The bearer token of the first connection, and the token that the mock
/// accepts after the change. They are different strings, so an incorrect
/// token in a request is visible and does not give a passing test.
const STALE_TOKEN: &str = "stale-access-token";
const FRESH_TOKEN: &str = "fresh-access-token";

/// The refresh token that the seeded account holds. A real account has it
/// after one consent, and a silent refresh is not possible without it.
const SEED_REFRESH_TOKEN: &str = "seed-refresh-token";

/// `[Authentication] User`, which is half of the secret-store key that EDS
/// derives (`eos_generate_secret_uid`, `e-oauth2-service.c:1086`): `"OAuth2::
/// <service name>[<user>]"`. The test gives the key to the client, so the
/// client and the test cannot use different keys.
const SECRET_USER: &str = "jmap-stale-token";

/// The key from `jmap_config::oauth2_service::NAME` (`"JMAP"`) and
/// [`SECRET_USER`].
const SECRET_UID: &str = "OAuth2::JMAP[jmap-stale-token]";

/// The `expires_in` the mock's token endpoint answers with, in seconds.
///
/// `e-oauth2-service.c` sets both limits. `eos_lookup_token_sync` reads the
/// stored token as `expires_after - now - 1`.
/// `e_oauth2_service_get_access_token_sync` rejects it at `<= 0` ("The access
/// token is expired and it failed to refresh it"), and refreshes at
/// `<= TOKEN_VALIDITY_GAP_SECS` (10). Thus a value in `2..=10` succeeds now
/// and refreshes on the next fetch. The value 8 gives the largest time margin:
/// the store and the read-back are consecutive statements, and they can be
/// up to seven seconds apart.
const TOKEN_LIFETIME_SECONDS: u64 = 8;

/// The maximum time that the harness waits for the client to connect.
const READY_TIMEOUT: Duration = Duration::from_secs(45);

/// The account: a standalone calendar, as in `calendar.rs` and the related
/// tests, with two additions for this test.
///
/// The first is `[Authentication] Method=JMAP`. Do not change this line.
/// `e_oauth2_services_is_oauth2_alias` matches it to the service that
/// `module-jmap-backend.so` and `libecalbackendjmap.so` each register. Thus
/// `source_uses_oauth2` is true, and the registry exports
/// `Source.OAuth2Support`.
///
/// The second is `[JMAP OAuth2]`, the extension of `jmap_config::oauth2`.
/// Our `EOAuth2Service::get_refresh_uri` returns its `TokenEndpoint`, so EDS
/// sends the refresh there. EDS makes the key names from the GObject property
/// names (`e_source_parameter_to_key`, `token-endpoint` → `TokenEndpoint`).
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP calendar stale-token functional test\n\
         Enabled=true\n\
         \n\
         [Calendar]\n\
         BackendName=jmap\n\
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

/// The token secret in the format of `e-oauth2-service.c`, with only the
/// refresh token. The module header tells why the other fields are absent.
fn seeded_secret() -> String {
    json!({ "refresh_token": SEED_REFRESH_TOKEN }).to_string()
}

/// One run of the client against a newly seeded mock. The accepted bearer
/// token changes while the client waits at its handshake.
///
/// `refresh_serves_fresh_token` is the only difference between the two tests:
/// whether the token endpoint then issues the token that the mock accepts.
/// With `false`, the refresh succeeds but the server refuses the result (a
/// revoked account). This must fail one time.
fn run(root: &str, refresh_serves_fresh_token: bool) -> Run {
    let client = required_path("JMAP_FUNCTIONAL_CAL_STALE_TOKEN_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");
    let collection_module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");
    let oauth2_services = required_path("JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE");

    // The token that `/oauth/token` issues, and its request log. The server
    // handler thread and the rotation thread share them.
    let issued_token = Arc::new(Mutex::new(STALE_TOKEN.to_owned()));
    let token_requests = Arc::new(Mutex::new(Vec::<BTreeMap<String, String>>::new()));

    let server = {
        let issued_token = Arc::clone(&issued_token);
        let token_requests = Arc::clone(&token_requests);
        jmap_mock::MockServer::builder()
            .bearer_token(STALE_TOKEN)
            .oauth_token(move |fields| {
                token_requests
                    .lock()
                    .expect("token request log")
                    .push(fields.clone());
                let access_token = issued_token.lock().expect("issued token").clone();
                (
                    200,
                    json!({
                        "access_token": access_token,
                        "token_type": "Bearer",
                        "expires_in": TOKEN_LIFETIME_SECONDS,
                        "refresh_token": SEED_REFRESH_TOKEN,
                    }),
                )
            })
            .start()
    };
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.seed_calendar("Personal", true);
    }

    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(root);
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);
    // The registry part. In the registry process, `module-jmap-backend.so`
    // registers the "JMAP" `EOAuth2Service` and the `[JMAP OAuth2]` extension
    // type. Without the extension type, the parser drops `TokenEndpoint`.
    // The EDS oauth2-services module exports `Source.OAuth2Support` for a
    // `Method=JMAP` source. Keep this order: both calls set
    // `EDS_REGISTRY_MODULES`, and the second adds to the first directory.
    session.stage_collection_backend(&collection_module);
    session.stage_installed_registry_module(&oauth2_services);

    // The two handshake files. Their paths come from a file that the session
    // creates, because only this gives a path in the session root. The anchor
    // is not a handshake file: the client uses the existence of the ready
    // file as the signal.
    let anchor = session.write_input("handshake-anchor", "");
    let ready_path = anchor.with_file_name("client-ready");
    let go_path = anchor.with_file_name("harness-go");

    let finished = AtomicBool::new(false);
    let output = thread::scope(|scope| {
        let rotator = scope.spawn(|| {
            rotate(
                &server,
                &issued_token,
                &ready_path,
                &go_path,
                &finished,
                refresh_serves_fresh_token,
            )
        });

        let output = session.run(
            &client,
            &[
                "jmap-functional",
                SECRET_UID,
                &seeded_secret(),
                &ready_path.to_string_lossy(),
                &go_path.to_string_lossy(),
            ],
        );
        // Release the rotator if the client stopped before its ready file.
        finished.store(true, Ordering::Release);
        rotator.join().expect("the rotating thread panicked");
        output
    });

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let token_requests = token_requests.lock().expect("token request log").clone();

    Run {
        status: output.status,
        stdout,
        report,
        unauthorized_responses: server.unauthorized_responses(),
        token_requests,
    }
}

/// The result of one [`run`]. The client stdout is separate from the report,
/// so [`observations`] reads only client observations. EDS writes many
/// warnings to stderr, and a warning with `=` is otherwise read as one.
struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    report: String,
    /// The number of requests that the mock refused with a 401. Without it,
    /// the client observations are the same for a run where the token did
    /// not become stale.
    unauthorized_responses: usize,
    /// Each form body that `/oauth/token` received, in order. These are the
    /// refreshes that EDS did, counted at the endpoint.
    token_requests: Vec<BTreeMap<String, String>>,
}

/// Wait for the `ready` file of the client, change the bearer token that the
/// mock accepts (and, in the positive case, the token that its endpoint
/// issues), then create `go`.
fn rotate(
    server: &jmap_mock::MockServer,
    issued_token: &Mutex<String>,
    ready_path: &Path,
    go_path: &Path,
    finished: &AtomicBool,
    refresh_serves_fresh_token: bool,
) {
    let deadline = Instant::now() + READY_TIMEOUT;
    while !ready_path.exists() {
        if finished.load(Ordering::Acquire) || Instant::now() > deadline {
            // No `go` file: the client stopped, or its handshake fails with
            // a better message than an assertion here.
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    // Use the real-world order: the server refuses the old token, then a new
    // token is available. In the opposite order, the backend can get the new
    // token early, and the test passes without a 401.
    server.set_bearer_token(FRESH_TOKEN);
    if refresh_serves_fresh_token {
        *issued_token.lock().expect("issued token") = FRESH_TOKEN.to_owned();
    }

    std::fs::write(go_path, "go").expect("write the harness's go file");
}

/// The preconditions of both tests: the account is of the expected kind, and
/// the run has data to measure. Without them, the counts in the tests have
/// no meaning.
fn assert_preconditions(run: &Run, seen: &BTreeMap<&str, &str>) {
    let report = &run.report;

    // The registry exports `Source.OAuth2Support` for this account. Without
    // it, the token fetch of the factory uses the in-process fallback of
    // `e-source.c`. In a calendar factory, that fallback has no `[JMAP OAuth2]`
    // extension and cannot give a refresh URI. The run then measures
    // something different.
    assert_eq!(
        seen.get("oauth2-support-exported"),
        Some(&"1"),
        "the registry did not export Source.OAuth2Support for an account whose \
         [Authentication] Method names our own EOAuth2Service\n{report}"
    );
    assert_eq!(
        seen.get("connection-status"),
        Some(&"connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert_eq!(
        seen.get("first-create-ok"),
        Some(&"1"),
        "the backend could not write at all, before any token went stale\n{report}"
    );
    assert_eq!(
        seen.get("credentials-required-before-rotation"),
        Some(&"0"),
        "EDS was already asking for credentials before the token went stale, so \
         the count afterwards says nothing about the rotation\n{report}"
    );
    assert_eq!(
        seen.get("done"),
        Some(&"1"),
        "the client did not run to the end\n{report}"
    );
    assert!(
        run.status.success(),
        "the client failed with {}\n{report}",
        run.status
    );

    // The connect fetched a token with a refresh against the token endpoint.
    // This shows that the credentials came from the seeded secret.
    let first = run
        .token_requests
        .first()
        .unwrap_or_else(|| panic!("nothing ever asked the token endpoint\n{report}"));
    assert_eq!(
        first.get("grant_type").map(String::as_str),
        Some("refresh_token"),
        "the connect's token fetch was not a refresh grant\n{report}"
    );
    assert_eq!(
        first.get("refresh_token").map(String::as_str),
        Some(SEED_REFRESH_TOKEN),
        "the refresh did not redeem the seeded refresh token\n{report}"
    );
}

/// The positive case: the stored refresh token gives a valid access token.
/// The operation must succeed silently.
#[test]
fn a_stale_token_is_refreshed_and_the_operation_retried_silently() {
    let run = run(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/cal-stale-token-refreshed"),
        true,
    );
    let seen = observations(&run.stdout);
    assert_preconditions(&run, &seen);
    let report = &run.report;

    // The main assertion. The server refused the token of the open
    // connection, and the user sees no effect.
    assert_eq!(
        seen.get("second-create-ok"),
        Some(&"1"),
        "the create after the token went stale failed instead of being retried\n{report}"
    );

    // `retry_on_authentication_failure` gives exactly two refreshes: one for
    // the connect and one for the retry. A third refresh is a loop.
    assert_eq!(
        run.token_requests.len(),
        2,
        "the retry did not take exactly one refresh; the token endpoint saw {:?}\n{report}",
        run.token_requests
    );

    // On the server side, exactly one request was refused. With zero, the
    // token did not become stale, and the counts above prove nothing.
    assert_eq!(
        run.unauthorized_responses, 1,
        "the mock refused {} requests, not the single stale-token one\n{report}",
        run.unauthorized_responses
    );

    // The part that the user sees: no consent window. In the application,
    // this signal opens the credentials prompter, which for a `Method=JMAP`
    // source is the consent window.
    assert_eq!(
        seen.get("credentials-required"),
        Some(&"0"),
        "a merely stale access token reached EDS's credentials prompter: in the \
         running application that is the hourly re-consent window this test exists to avoid\n{report}"
    );
}

/// The negative case: the refresh succeeds, but the server refuses the new
/// token (a revoked account, or withdrawn credentials). This must fail after
/// exactly one refresh. A retry loop is worse than no retry, because it
/// shows the re-consent window again and again.
#[test]
fn a_refresh_that_does_not_help_fails_once_rather_than_looping() {
    let run = run(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/cal-stale-token-unhelped"),
        false,
    );
    let seen = observations(&run.stdout);
    assert_preconditions(&run, &seen);
    let report = &run.report;

    assert_eq!(
        seen.get("second-create-ok"),
        Some(&"0"),
        "the create succeeded even though no token the server accepts was ever \
         available\n{report}"
    );

    // Two refreshes: one for the connect and one for the retry. A retry loop
    // increases this count. The client counters cannot show a second retry.
    assert_eq!(
        run.token_requests.len(),
        2,
        "the failing refresh was attempted more than once, or not at all; the \
         token endpoint saw {:?}\n{report}",
        run.token_requests
    );

    // Two refusals: the create and its retry.
    assert_eq!(
        run.unauthorized_responses, 2,
        "the mock refused {} requests, not the attempt and its single retry\n{report}",
        run.unauthorized_responses
    );

    // The user gets an error message. The tests of `jmap-backend-cal`
    // examine its text.
    assert!(
        seen.get("second-create-error")
            .is_some_and(|message| !message.is_empty()),
        "the failed create reported no error message at all\n{report}"
    );
}
