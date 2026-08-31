// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The CALENDAR leg: item 23's acceptance test, made headless for the
//! backend that refreshes through an `ESource` rather than through a
//! `CamelSession`.
//!
//! The mail leg (`mail-stale-token.rs`) got to override
//! `get_oauth2_access_token_sync` on a test `CamelSession`, because
//! `jmap_mail::oauth2::uses_oauth2` reads Camel's own `auth-mechanism` field
//! and the generic `[Authentication] Method=OAuth2` satisfies it. The calendar
//! backend has no such shortcut:
//! `jmap_backend_core::oauth2::source_uses_oauth2` goes through
//! `e_oauth2_services_is_oauth2_alias`, which matches only a `Method` naming
//! an `EOAuth2Service` registered in the asking process — so the account must
//! say `Method=JMAP`, and that is precisely the condition under which EDS's
//! own `module-oauth2-services.so` exports `Source.OAuth2Support` (item 22).
//! The refresh therefore runs for real, in the registry, through this
//! project's own `EOAuth2Service` and against a real token endpoint. There is
//! no stand-in anywhere in the chain.
//!
//! ## What makes the refresh happen, without a wall clock
//!
//! N+90's trace expected this to hinge on a seeded `expires_after` falling due
//! between two fetches — real timing, and fragile. It does not.
//! `e_oauth2_service_get_access_token_sync` refreshes whenever the token it
//! looked up has `expires_in <= TOKEN_VALIDITY_GAP_SECS` (**10**,
//! `e-oauth2-service.c:47`), and `eos_lookup_token_sync` derives that number
//! from the stored `expires_after` — absent, it stays at `-1`. So a seeded
//! secret carrying only a `refresh_token` makes *every* fetch go to the token
//! endpoint, and [`TOKEN_LIFETIME_SECONDS`] keeps it that way for the tokens
//! the endpoint then issues. The mock is the single authority on which access
//! token exists, exactly as `mail-stale-token.rs`'s token file is for mail.
//!
//! ## What each test holds the backend to
//!
//! Both drive the same program through the same steps — connect, create an
//! event, then create another after the server has changed its mind about
//! which bearer token it accepts — and differ in one thing: whether the token
//! endpoint hands out the new one.
//!
//! The interesting assertions are counts taken from the server's own side of
//! the wire, because a backend that answered the second create by tearing the
//! connection down and going back to EDS for credentials would also, in the
//! positive case, eventually produce an event.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use jmap_functional::{Session, observations, required_path};
use serde_json::json;

/// The bearer token the account connects with, and the one the mock is
/// switched to while that connection is open. Distinct strings rather than a
/// counter, so a mock that had somehow kept accepting the first one shows up
/// as the wrong token in a request rather than as a passing test.
const STALE_TOKEN: &str = "stale-access-token";
const FRESH_TOKEN: &str = "fresh-access-token";

/// The refresh token the account is seeded as already holding — what a real
/// account has after being consented to once, and the only thing that makes a
/// silent refresh possible at all.
const SEED_REFRESH_TOKEN: &str = "seed-refresh-token";

/// `[Authentication] User`, and therefore half of the secret-store key EDS
/// derives (`eos_generate_secret_uid`, `e-oauth2-service.c:1086`): `"OAuth2::
/// <service name>[<user>]"`. Stated here, and handed to the client, rather
/// than built in the client — a client that derived it could drift from the
/// test that asserts on it.
const SECRET_USER: &str = "jmap-stale-token";

/// `jmap_config::oauth2_service::NAME` is `"JMAP"`; this is the key that name
/// and [`SECRET_USER`] produce.
const SECRET_UID: &str = "OAuth2::JMAP[jmap-stale-token]";

/// The `expires_in` the mock's token endpoint answers with, in seconds.
///
/// Bounded on both sides by `e-oauth2-service.c`, and both bounds matter:
/// `eos_lookup_token_sync` reads the stored token back as
/// `expires_after - now - 1`, which `e_oauth2_service_get_access_token_sync`
/// rejects outright at `<= 0` ("The access token is expired and it failed to
/// refresh it"), and refreshes again at `<= TOKEN_VALIDITY_GAP_SECS` (10).
/// Anything in `2..=10` therefore succeeds now and still refreshes next time;
/// 8 leaves the widest margin on the side that would otherwise be timing —
/// seven seconds may pass between the store and the read-back before this
/// stops working, and they are consecutive statements in one function.
const TOKEN_LIFETIME_SECONDS: u64 = 8;

/// How long the harness waits for the client to say its connection is up.
const READY_TIMEOUT: Duration = Duration::from_secs(45);

/// The account: a standalone calendar, as `calendar.rs` and its siblings use,
/// with two additions this test is entirely about.
///
/// `[Authentication] Method=JMAP` is the first, and the one line that must not
/// drift: it is what `e_oauth2_services_is_oauth2_alias` matches against the
/// service `module-jmap-backend.so` and `libecalbackendjmap.so` each register,
/// and so both what makes the backend's `source_uses_oauth2` true and what
/// makes the registry export `Source.OAuth2Support`. `Method=OAuth2` — the
/// generic spelling the mail leg can use — names no registered service and
/// would produce an account this test's whole chain declines to touch.
///
/// `[JMAP OAuth2]` is the second: `jmap_config::oauth2`'s extension, whose
/// `TokenEndpoint` is what this project's `EOAuth2Service::get_refresh_uri`
/// answers with and therefore where EDS sends the refresh. The key names are
/// EDS's own transformation of the GObject property names
/// (`e_source_parameter_to_key`, `token-endpoint` → `TokenEndpoint`).
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

/// The secret `e_oauth2_service.c` stores tokens as, carrying only the refresh
/// token — see this module's header for why the omissions are the mechanism
/// rather than laziness.
fn seeded_secret() -> String {
    json!({ "refresh_token": SEED_REFRESH_TOKEN }).to_string()
}

/// One run of the client against a freshly seeded mock, with the accepted
/// bearer token rotated underneath it while it waits at its handshake.
///
/// `refresh_serves_fresh_token` is the one thing the two tests differ in:
/// whether the mock's token endpoint starts answering with the token the mock
/// now accepts. `false` is a stored refresh token that still redeems but no
/// longer buys anything the server will take — a revoked account — which must
/// fail, once.
fn run(root: &str, refresh_serves_fresh_token: bool) -> Run {
    let client = required_path("JMAP_FUNCTIONAL_CAL_STALE_TOKEN_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");
    let collection_module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");
    let oauth2_services = required_path("JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE");

    // What `/oauth/token` hands out, and what it has been asked. Shared with
    // the server's handler thread and with the rotating thread below.
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
    // The registry's half. `module-jmap-backend.so` is what registers both the
    // "JMAP" `EOAuth2Service` and `[JMAP OAuth2]`'s extension type in the
    // registry process — the second is why the source's `TokenEndpoint`
    // survives being parsed at all — and EDS's own oauth2-services module is
    // what turns a `Method=JMAP` source into one with `Source.OAuth2Support`
    // exported. Order matters: both write `EDS_REGISTRY_MODULES`, and the
    // second adds to the directory the first created.
    session.stage_collection_backend(&collection_module);
    session.stage_installed_registry_module(&oauth2_services);

    // The two files the two sides meet at. Named relative to a file the
    // session will actually create, because that is the only way this crate
    // hands out a path inside the session root — and the anchor itself is
    // deliberately *not* one of them: the client treats the ready file's mere
    // existence as the signal.
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
        // Releases the rotator if the client died before ever writing its
        // ready file.
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

/// What one [`run`] produced. The client's stdout is kept apart from the
/// report so that [`observations`] reads only the lines the client meant as
/// observations — EDS is talkative on stderr, and a warning containing an `=`
/// would otherwise be read as one.
struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    report: String,
    /// How many requests the mock refused with a 401 across the whole run.
    /// Without it the client-side observations could all read the same for a
    /// run in which the token never actually went stale.
    unauthorized_responses: usize,
    /// Every form body `/oauth/token` was asked with, in order — the refreshes
    /// EDS actually performed, counted where they happened rather than
    /// inferred from what the backend did afterwards.
    token_requests: Vec<BTreeMap<String, String>>,
}

/// Wait for the client's `ready`, change which bearer token the mock accepts
/// (and, in the positive case, which one its token endpoint issues), then
/// create `go`.
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
            // No `go` file: the client is either gone already or is about to
            // fail its own handshake and say so, which is a better failure
            // than one asserted from here.
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    // The order is the real-world one: the server stops accepting the old
    // token, and only then is a new one obtainable. Reversed, the backend
    // could be handed the fresh token before the mock had a use for it and the
    // test would pass without ever producing a 401.
    server.set_bearer_token(FRESH_TOKEN);
    if refresh_serves_fresh_token {
        *issued_token.lock().expect("issued token") = FRESH_TOKEN.to_owned();
    }

    std::fs::write(go_path, "go").expect("write the harness's go file");
}

/// The preconditions both tests share: that the account this test built is the
/// kind of account it thinks it is, and that the run got as far as having
/// something to measure. Every count below either test's call to this is
/// meaningless without them.
fn assert_preconditions(run: &Run, seen: &BTreeMap<&str, &str>) {
    let report = &run.report;

    // First, and the one an earlier session doubted for the mail-side
    // equivalent: our account really is one the registry exports
    // `Source.OAuth2Support` for. Without it the factory's token fetch takes
    // `e-source.c`'s in-process fallback, which in a calendar factory finds no
    // `[JMAP OAuth2]` extension registered and could not produce a refresh URI
    // at all — the run would still happen, and would measure something else.
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

    // The connect fetched exactly one token, and it did so as a refresh
    // against a real token endpoint — which is what says the seeded secret,
    // not something else, is where the backend's credentials came from.
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

/// The positive case, and the one item 23 is about: a stored refresh token
/// that still buys a working access token. The operation the user asked for
/// must simply happen.
#[test]
fn a_stale_token_is_refreshed_and_the_operation_retried_silently() {
    let run = run(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/cal-stale-token-refreshed"),
        true,
    );
    let seen = observations(&run.stdout);
    assert_preconditions(&run, &seen);
    let report = &run.report;

    // The whole point. The server has refused the token the open connection
    // carries, and the user notices nothing.
    assert_eq!(
        seen.get("second-create-ok"),
        Some(&"1"),
        "the create after the token went stale failed instead of being retried\n{report}"
    );

    // How it succeeded, not merely that it did. Exactly two refreshes — the
    // connect's and the retry's — is what `retry_on_authentication_failure`
    // promises; a third would be a loop.
    assert_eq!(
        run.token_requests.len(),
        2,
        "the retry did not take exactly one refresh; the token endpoint saw {:?}\n{report}",
        run.token_requests
    );

    // And from the server's own side of the wire, the assertion that makes the
    // rest mean anything: exactly one request was refused. Zero would be a run
    // in which the token never went stale at all — every count above would
    // still read the same, and this test would be asserting nothing.
    assert_eq!(
        run.unauthorized_responses, 1,
        "the mock refused {} requests, not the single stale-token one\n{report}",
        run.unauthorized_responses
    );

    // The user-visible half of item 23: no consent window. In the running
    // application this signal is what raises the credentials prompter, and for
    // a `Method=JMAP` source that is the consent window itself.
    assert_eq!(
        seen.get("credentials-required"),
        Some(&"0"),
        "a merely stale access token reached EDS's credentials prompter — in the \
         running application that is the hourly consent window item 23 is about\n{report}"
    );
}

/// The negative case: the refresh redeems, but what it buys the server also
/// refuses — a revoked account, or credentials that have been withdrawn behind
/// the client's back. That has to fail, and to fail after exactly one attempt
/// at refreshing. A retry loop here is worse than no retry at all: it is the
/// consent window item 23 is about, arriving repeatedly.
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

    // Two refreshes, not more: the connect's and the single retry's. This is
    // the count that would climb if the retry looped, and the counters the
    // client reports could not tell a second retry apart from anything else.
    assert_eq!(
        run.token_requests.len(),
        2,
        "the failing refresh was attempted more than once, or not at all; the \
         token endpoint saw {:?}\n{report}",
        run.token_requests
    );

    // Two refusals: the create, and the one retry it was given.
    assert_eq!(
        run.unauthorized_responses, 2,
        "the mock refused {} requests, not the attempt and its single retry\n{report}",
        run.unauthorized_responses
    );

    // The user is told something rather than nothing. Which message it is
    // belongs to `jmap-backend-cal`'s own tests; that there is one is this
    // test's.
    assert!(
        seen.get("second-create-error")
            .is_some_and(|message| !message.is_empty()),
        "the failed create reported no error message at all\n{report}"
    );
}
