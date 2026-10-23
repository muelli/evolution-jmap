// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The address book part of the stale-access-token acceptance test, without
//! a display, for `jmap-backend-book`.
//!
//! This is `cal-stale-token.rs` with the address book backend and client in
//! place of the calendar ones. Both backends use the same
//! `jmap_backend_core::oauth2::source_uses_oauth2`. Thus all constraints in
//! the header of that file apply: `Method=JMAP`, the refresh from the
//! validity gap without a wall clock, and the two staged registry modules.
//! That header gives the full mechanism.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use serde_json::json;

/// See `cal-stale-token.rs`.
const STALE_TOKEN: &str = "stale-access-token";
const FRESH_TOKEN: &str = "fresh-access-token";
const SEED_REFRESH_TOKEN: &str = "seed-refresh-token";
const SECRET_USER: &str = "jmap-stale-token";
const SECRET_UID: &str = "OAuth2::JMAP[jmap-stale-token]";
const TOKEN_LIFETIME_SECONDS: u64 = 8;
const READY_TIMEOUT: Duration = Duration::from_secs(45);

/// The account: a standalone address book, with the two OAuth2 additions
/// that the `keyfile` of `cal-stale-token.rs` describes.
fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::AddressBook, port)
        .display_name("JMAP address book stale-token functional test")
        .user(SECRET_USER)
        .auth_method("JMAP")
        .oauth2()
        .to_string()
}

/// See `cal-stale-token.rs`.
fn seeded_secret() -> String {
    json!({ "refresh_token": SEED_REFRESH_TOKEN }).to_string()
}

/// One run of the client against a newly seeded mock. The accepted bearer
/// token changes while the client waits at its handshake. The `run` of
/// `cal-stale-token.rs` describes `refresh_serves_fresh_token`.
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
    // The registry part. The same calls in `cal-stale-token.rs` tell why both
    // are necessary, in this order.
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

/// See `Run` in `cal-stale-token.rs`.
struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    report: String,
    unauthorized_responses: usize,
    token_requests: Vec<BTreeMap<String, String>>,
}

/// See `rotate` in `cal-stale-token.rs`.
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

/// See `assert_preconditions` in `cal-stale-token.rs`.
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

/// The positive case: the stored refresh token gives a valid access token.
/// The operation must succeed silently.
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
        "a merely stale access token reached EDS's credentials prompter, which in \
         the running application is the hourly consent window this test exists to \
         avoid\n{report}"
    );
}

/// The negative case: the refresh succeeds, but the server refuses the new
/// token. The operation must fail one time and not loop.
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
