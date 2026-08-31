// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 23's acceptance test, made headless.
//!
//! Item 23 — the hourly re-consent — is code-complete at all four call sites,
//! and its acceptance test was "the operator leaves Evolution open for hours
//! and sees no consent window": slow, unrepeatable, and silently invalid if
//! the host suspends. Its parting note says a real test cannot be built,
//! "because the refresh path needs a real `CamelService` registered on a real
//! `CamelSession` (`EMailSession`), which no headless test on this VM can
//! build".
//!
//! It can. `tests/functional/mail-client.c` has been building a real
//! `CamelService` since the mail leg landed — the store Camel opens after
//! finding the provider through `libcameljmap.urls`, in the process the
//! provider is dlopened into. What a plain `CamelSession` does not do is
//! answer `get_oauth2_access_token_sync`, and that is a class method a test
//! session can override the same way `EMailSession` overrides it. See
//! `tests/functional/mail-stale-token-client.c`.
//!
//! ## What each test holds the provider to
//!
//! Both drive the same program through the same three steps — connect, list,
//! then list again after the server has changed its mind about which bearer
//! token it accepts — and differ in one thing: whether the token the session
//! can fetch is the new one.
//!
//! The interesting assertion in both is a *count*, not a success. A provider
//! that answered the second listing by tearing the connection down and going
//! back to the session for credentials would also produce a folder tree, and
//! would be exactly the bug item 23 is about.
//!
//! `camel_session_authenticate_sync` is where that shows. `jmap-mail`'s
//! `connect_sync` (`service.rs`) authenticates an OAuth 2.0 account *itself*,
//! through `camel_service_authenticate_sync`, and reaches the session's
//! interactive loop only when the server rejects the token outright — which
//! in the running application is `mail_ui_session_authenticate_sync`, the
//! credentials prompter, and for an OAuth2-method source the consent window.
//! So the session's `authenticate_sync` is not merely a proxy for the
//! escalation: on this path it *is* the escalation, and a healthy run leaves
//! its count at zero.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use jmap_functional::{Session, observations, required_path};

/// The bearer token the account connects with, and the one the mock is
/// switched to while that connection is open. Distinct strings rather than a
/// counter, so a mock that had somehow kept accepting the first one shows up
/// as the wrong token in a request rather than as a passing test.
const STALE_TOKEN: &str = "stale-access-token";
const FRESH_TOKEN: &str = "fresh-access-token";

/// How long the harness waits for the client to say its connection is up.
/// Generous for the same reason the ctest timeout is: it covers activating
/// the registry and a first connect.
const READY_TIMEOUT: Duration = Duration::from_secs(45);

/// `docs/examples/jmap-mock-standalone-mail.source`, with the mock's
/// ephemeral port filled in and one line changed: `[Authentication] Method`,
/// which EDS binds to `CamelNetworkSettings:auth-mechanism` and which
/// `jmap_mail::oauth2::uses_oauth2` reads to decide this is an OAuth 2.0
/// account. `OAuth2` is the generic spelling
/// (`jmap_backend_core::oauth2::OAUTH2_METHOD`) rather than the name of a
/// registered `EOAuth2Service`: no service has to exist for the provider to
/// know it should ask its session for a token, and asking the session is the
/// whole of what this test is about.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP stale-token functional test\n\
         Enabled=true\n\
         \n\
         [Mail Account]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         Method=OAuth2\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// One run of the client against a freshly seeded mock, with the token
/// rotated underneath it while it waits at its handshake.
///
/// `refresh_serves_fresh_token` is the one thing the two tests differ in:
/// whether the file the client's session answers `get_oauth2_access_token_
/// sync` out of is updated along with the mock. `false` is a stored refresh
/// token that no longer works — the case that must still escalate, and must
/// do so exactly once.
fn run(root: &str, refresh_serves_fresh_token: bool) -> Run {
    let client = required_path("JMAP_FUNCTIONAL_MAIL_STALE_TOKEN_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_MAIL_MODULE");
    let urls = required_path("JMAP_FUNCTIONAL_MAIL_URLS");

    let server = jmap_mock::MockServer::builder()
        .bearer_token(STALE_TOKEN)
        .start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.seed_mailbox("Inbox", Some("inbox"));
        account.seed_mailbox("Sent", Some("sent"));
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
    session.stage_camel_provider(&module, &urls);

    // The token the client's session hands out, and the two files the two
    // sides meet at. All three live beside each other under the session root,
    // which the next run wipes.
    let token_path = session.write_input("access-token", STALE_TOKEN);
    let ready_path = token_path.with_file_name("client-ready");
    let go_path = token_path.with_file_name("harness-go");

    // The rotation happens while `Session::run` blocks, so it happens on a
    // thread. Everything it touches is either owned by it or `Sync`: the mock
    // handle is shared by reference across the scope, and the two paths are
    // its own.
    let finished = AtomicBool::new(false);
    let output = thread::scope(|scope| {
        let rotator = scope.spawn(|| {
            rotate(
                &server,
                &token_path,
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
                &token_path.to_string_lossy(),
                &ready_path.to_string_lossy(),
                &go_path.to_string_lossy(),
            ],
        );
        // Releases the rotator if the client died before ever writing its
        // ready file — otherwise this scope would block until the timeout for
        // a client that is already gone.
        finished.store(true, Ordering::Release);
        rotator.join().expect("the rotating thread panicked");
        output
    });

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");

    Run {
        status: output.status,
        stdout,
        report,
        unauthorized_responses: server.unauthorized_responses(),
    }
}

/// What one [`run`] produced. The client's stdout is kept apart from the
/// report so that [`observations`] reads only the lines the client meant as
/// observations — EDS and Camel are talkative on stderr, and a warning
/// containing an `=` would otherwise be read as one.
struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    report: String,
    /// How many requests the mock refused with a 401 across the whole run —
    /// the server's own count of the thing being provoked, from the far side
    /// of the wire. Without it the counts the client reports could all be
    /// right for a run in which the token never actually went stale.
    unauthorized_responses: usize,
}

/// Wait for the client's `ready`, change which bearer token the mock accepts
/// (and, in the positive case, which one the client's session will hand
/// back), then create `go`.
fn rotate(
    server: &jmap_mock::MockServer,
    token_path: &Path,
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

    // The order matters and is the real-world one: the server stops accepting
    // the old token, and only then is a new one obtainable. Reversed, the
    // client could fetch the fresh token before the mock had a use for it and
    // the test would pass without ever producing a 401.
    server.set_bearer_token(FRESH_TOKEN);
    if refresh_serves_fresh_token {
        std::fs::write(token_path, FRESH_TOKEN).expect("write the refreshed access token");
    }

    std::fs::write(go_path, "go").expect("write the harness's go file");
}

/// The positive case, and the one item 23 is about: a stored refresh token
/// that still works. The operation the user asked for must simply happen.
#[test]
fn a_stale_token_is_refreshed_and_the_operation_retried_silently() {
    let Run {
        status,
        stdout,
        report,
        unauthorized_responses,
    } = run(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/mail-stale-token-refreshed"),
        true,
    );
    let seen = observations(&stdout);

    // Before anything else, because it is what says the account this test
    // built is the kind of account it thinks it is. `uses_oauth2` reads this
    // field; if the keyfile's `[Authentication] Method` did not reach it,
    // every observation below would be about a password account.
    assert_eq!(
        seen.get("auth-mechanism"),
        Some(&"OAuth2"),
        "the keyfile's [Authentication] Method never reached the Camel settings\n{report}"
    );
    assert_eq!(
        seen.get("store-connected"),
        Some(&"1"),
        "Camel never opened the store\n{report}"
    );
    assert!(
        status.success(),
        "the client failed with {status}\n{report}",
    );

    // The connect fetched exactly one token, through the session — which is
    // already more than the old unit tests could see: it is the provider
    // asking a real `CamelSession` for a real account's token.
    assert_eq!(
        seen.get("token-fetches-before-rotation"),
        Some(&"1"),
        "the connect did not fetch exactly one access token through the session\n{report}"
    );
    // And it did so without the session's interactive loop, which is what
    // `connect_sync`'s own silent-attempt-first branch exists for: a connect
    // that already needed the prompter would make the count after the
    // rotation impossible to read.
    assert_eq!(
        seen.get("authenticate-calls-before-rotation"),
        Some(&"0"),
        "the connect went through the session's interactive authentication\n{report}"
    );
    assert_eq!(
        seen.get("folders"),
        Some(&"Inbox,Sent"),
        "the first listing is not the mock's two mailboxes\n{report}"
    );

    // The whole point. The server has refused the token the open connection
    // carries, and the user notices nothing.
    assert_eq!(
        seen.get("second-listing-ok"),
        Some(&"1"),
        "the listing after the token went stale failed instead of being retried\n{report}"
    );
    assert_eq!(
        seen.get("folders-after-rotation"),
        Some(&"Inbox,Sent"),
        "the retried listing did not answer with the mock's mailboxes\n{report}"
    );

    // How it succeeded, not merely that it did. One extra token fetch is a
    // refresh; a second `authenticate` call would be the provider going back
    // to the session for credentials, which in the running application is the
    // consent window item 23 exists to stop.
    assert_eq!(
        seen.get("token-fetches"),
        Some(&"2"),
        "the retry did not take exactly one refresh\n{report}"
    );
    assert_eq!(
        seen.get("authenticate-calls"),
        Some(&"0"),
        "the 401 reached the session's interactive authentication — in the running \
         application that is the consent window item 23 is about\n{report}"
    );

    // From the server's own side of the wire, and the assertion that makes
    // the rest of them mean anything: exactly one request was refused. Zero
    // would be a run in which the token never went stale at all — every
    // count above would still read the same, and the test would be asserting
    // nothing.
    assert_eq!(
        unauthorized_responses, 1,
        "the mock refused {unauthorized_responses} requests, not the single stale-token one\n{report}"
    );
}

/// The negative case: the refresh produces a token the server also refuses —
/// a revoked account, or a refresh token that has itself expired. That has to
/// fail, and it has to fail after exactly one attempt at refreshing. A retry
/// loop here is worse than no retry at all: it is the consent window item 23
/// is about, arriving repeatedly.
#[test]
fn a_refresh_that_does_not_help_fails_once_rather_than_looping() {
    let Run {
        status,
        stdout,
        report,
        unauthorized_responses,
    } = run(
        concat!(env!("CARGO_TARGET_TMPDIR"), "/mail-stale-token-unhelped"),
        false,
    );
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("store-connected"),
        Some(&"1"),
        "Camel never opened the store\n{report}"
    );
    assert!(
        status.success(),
        "the client failed with {status}\n{report}",
    );

    assert_eq!(
        seen.get("second-listing-ok"),
        Some(&"0"),
        "the listing succeeded even though no token the server accepts was ever available\n{report}"
    );
    assert_eq!(
        seen.get("token-fetches"),
        Some(&"2"),
        "the failing refresh was attempted more than once, or not at all\n{report}"
    );

    // Two refusals, not more: the listing, and the one retry it was given.
    // A provider that kept retrying would climb this count, which is the
    // failure mode this test exists for — the counters the client reports
    // could not tell a second retry apart from a second token fetch.
    assert_eq!(
        unauthorized_responses, 2,
        "the mock refused {unauthorized_responses} requests, not the attempt and its single retry\n{report}"
    );

    // The user is told something rather than nothing. Which message it is
    // belongs to `jmap-mail`'s own tests; that there is one is this test's.
    assert!(
        seen.get("second-listing-error")
            .is_some_and(|message| !message.is_empty()),
        "the failed listing reported no error message at all\n{report}"
    );

    // "Escalates exactly once" — and this is where the count says something
    // this test did not expect going in, so it is asserted rather than
    // described: the escalation is *not* inside the failing operation. The
    // store reports the 401 to whoever asked for the folder tree and stops;
    // nothing goes back to the session. Which means the prompter is reached,
    // at most, once per reconnect rather than once per failed operation —
    // strictly better than "exactly once", and the assertion here is what
    // would notice if that ever changed.
    assert_eq!(
        seen.get("authenticate-calls"),
        Some(&"0"),
        "the failing operation went back to the session for credentials instead of \
         reporting the failure to its caller\n{report}"
    );
}
