// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `docs/ROADMAP.md` item 25, the ADDRESS-BOOK leg: item 23's acceptance
//! test, made headless for `jmap-backend-book`.
//!
//! This is `cal-stale-token.rs` with the calendar pair swapped for the
//! address-book one — `jmap_backend_core::oauth2::source_uses_oauth2` is the
//! same function for both backends, so every constraint that test's header
//! documents (the `Method=JMAP` requirement, the gap-driven refresh with no
//! wall clock, the two staged registry modules beside the backend's own)
//! applies unchanged. See that file's header for the full mechanism.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use jmap_functional::{Session, observations, required_path};
use serde_json::json;

/// See `cal-stale-token.rs`.
const STALE_TOKEN: &str = "stale-access-token";
const FRESH_TOKEN: &str = "fresh-access-token";
const SEED_REFRESH_TOKEN: &str = "seed-refresh-token";
const SECRET_USER: &str = "jmap-stale-token";
const SECRET_UID: &str = "OAuth2::JMAP[jmap-stale-token]";
const TOKEN_LIFETIME_SECONDS: u64 = 8;
const READY_TIMEOUT: Duration = Duration::from_secs(45);

/// The account: a standalone address book, with the same two OAuth2
/// additions `cal-stale-token.rs`'s `keyfile` documents in full.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP address book stale-token functional test\n\
         Enabled=true\n\
         \n\
         [Address Book]\n\
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

/// See `cal-stale-token.rs`.
fn seeded_secret() -> String {
    json!({ "refresh_token": SEED_REFRESH_TOKEN }).to_string()
}

/// One run of the client against a freshly seeded mock, with the accepted
/// bearer token rotated underneath it while it waits at its handshake. See
/// `cal-stale-token.rs`'s own copy for what `refresh_serves_fresh_token`
/// means.
fn run(root: &str, refresh_serves_fresh_token: bool) -> Run {
    let client = required_path("JMAP_FUNCTIONAL_BOOK_STALE_TOKEN_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");
    let collection_module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");
    let oauth2_services = required_path("JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE");

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
        account.seed_address_book("Personal", true);
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
    session.stage_address_book_backend(&module);
    // The registry's half — see `cal-stale-token.rs`'s comment on the same
    // two calls for why both are needed and in this order.
    session.stage_collection_backend(&collection_module);
    session.stage_installed_registry_module(&oauth2_services);

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

/// See `cal-stale-token.rs`'s `Run`.
struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    report: String,
    unauthorized_responses: usize,
    token_requests: Vec<BTreeMap<String, String>>,
}

/// See `cal-stale-token.rs`'s `rotate`.
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
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    server.set_bearer_token(FRESH_TOKEN);
    if refresh_serves_fresh_token {
        *issued_token.lock().expect("issued token") = FRESH_TOKEN.to_owned();
    }

    std::fs::write(go_path, "go").expect("write the harness's go file");
}

/// See `cal-stale-token.rs`'s `assert_preconditions`.
fn assert_preconditions(run: &Run, seen: &BTreeMap<&str, &str>) {
    let report = &run.report;

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

/// The positive case: a stored refresh token that still buys a working
/// access token. The operation the user asked for must simply happen.
#[test]
fn a_stale_token_is_refreshed_and_the_operation_retried_silently() {
    let run = run(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/book-stale-token-refreshed"),
        true,
    );
    let seen = observations(&run.stdout);
    assert_preconditions(&run, &seen);
    let report = &run.report;

    assert_eq!(
        seen.get("second-create-ok"),
        Some(&"1"),
        "the create after the token went stale failed instead of being retried\n{report}"
    );

    assert_eq!(
        run.token_requests.len(),
        2,
        "the retry did not take exactly one refresh; the token endpoint saw {:?}\n{report}",
        run.token_requests
    );

    assert_eq!(
        run.unauthorized_responses, 1,
        "the mock refused {} requests, not the single stale-token one\n{report}",
        run.unauthorized_responses
    );

    assert_eq!(
        seen.get("credentials-required"),
        Some(&"0"),
        "a merely stale access token reached EDS's credentials prompter — in the \
         running application that is the hourly consent window item 23 is about\n{report}"
    );
}

/// The negative case: the refresh redeems, but what it buys the server also
/// refuses. Must fail once, not loop.
#[test]
fn a_refresh_that_does_not_help_fails_once_rather_than_looping() {
    let run = run(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/book-stale-token-unhelped"),
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

    assert_eq!(
        run.token_requests.len(),
        2,
        "the failing refresh was attempted more than once, or not at all; the \
         token endpoint saw {:?}\n{report}",
        run.token_requests
    );

    assert_eq!(
        run.unauthorized_responses, 2,
        "the mock refused {} requests, not the attempt and its single retry\n{report}",
        run.unauthorized_responses
    );

    assert!(
        seen.get("second-create-error")
            .is_some_and(|message| !message.is_empty()),
        "the failed create reported no error message at all\n{report}"
    );
}
