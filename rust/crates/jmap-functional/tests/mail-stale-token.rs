// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The headless acceptance test for the hourly re-consent fix.
//!
//! The manual acceptance test was "leave Evolution open for hours and see no
//! consent window". That test is slow, not repeatable, and not valid if the
//! host suspends. An earlier analysis said that a headless test needs a real
//! `CamelService` on a real `CamelSession` (`EMailSession`), and that this VM
//! cannot build one.
//!
//! This is not correct. `tests/functional/mail-client.c` builds a real
//! `CamelService`: the store that Camel opens after it finds the provider
//! through `libcameljmap.urls`, in the process that loads the provider. A
//! plain `CamelSession` does not answer `get_oauth2_access_token_sync`, but a
//! test session can override this class method as `EMailSession` does. See
//! `tests/functional/mail-stale-token-client.c`.
//!
//! ## What each test examines
//!
//! The two tests run the same program through the same three steps: connect,
//! list, then list again after the server changes the bearer token that it
//! accepts. The only difference is whether the session can fetch the new
//! token.
//!
//! The important assertion in the two tests is a *count*, not a success. A
//! provider that closes the connection and asks the session for credentials
//! again also gives a folder tree. That is the bug under test.
//!
//! `camel_session_authenticate_sync` shows the bug. `connect_sync` of
//! `jmap-mail` (`service.rs`) names [`jmap_mail::sasl::MECHANISM`] and calls
//! `camel_session_authenticate_sync`, as `mail_ui_session_authenticate_sync`
//! does in the application. A mechanism that needs no password gets a silent
//! attempt at the `authenticate_sync` of the provider before a prompt. Thus a
//! correct connect calls the session one time. Only a rejected attempt goes on
//! to the credentials prompter, which is the consent window for an
//! OAuth2-method source. The baseline is one session-level call at connect
//! time. The escalation under test is each call *after* that baseline.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use jmap_functional::{Session, observations, required_path};

/// The bearer token of the first connect, and the token that the mock
/// accepts after the rotation. Different strings make a mock that still
/// accepts the first token visible as a wrong token in a request.
const STALE_TOKEN: &str = "stale-access-token";
const FRESH_TOKEN: &str = "fresh-access-token";

/// The maximum time that the harness waits for the client to connect. It
/// includes the activation of the registry and the first connect.
const READY_TIMEOUT: Duration = Duration::from_secs(45);

/// `docs/examples/jmap-mock-standalone-mail.source`, with the ephemeral port
/// of the mock and a changed `[Authentication] Method`. EDS binds that field
/// to `CamelNetworkSettings:auth-mechanism`, and
/// `jmap_mail::oauth2::uses_oauth2` reads it to identify an OAuth 2.0 account.
/// `OAuth2` is the generic name (`jmap_backend_core::oauth2::OAUTH2_METHOD`),
/// not a registered `EOAuth2Service`. The provider asks its session for a
/// token without a registered service, and this test examines only that step.
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

/// Runs the client against a new mock, and rotates the token while the
/// client waits at its handshake.
///
/// `refresh_serves_fresh_token` is the only difference between the two
/// tests. It sets whether the harness also updates the file from which the
/// client session answers `get_oauth2_access_token_sync`. `false` is a stored
/// refresh token that does not operate. That case must escalate exactly once.
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

    // The token file of the client session, and the two handshake files.
    // All three are in the session root, which the next run deletes.
    let token_path = session.write_input("access-token", STALE_TOKEN);
    let ready_path = token_path.with_file_name("client-ready");
    let go_path = token_path.with_file_name("harness-go");

    // The rotation runs on a thread because `Session::run` blocks. The
    // thread borrows the mock handle and the paths across the scope.
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
        // Release the rotator if the client stopped before it wrote its ready
        // file. Otherwise the scope blocks until the timeout.
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

/// The result of one [`run`]. The client stdout is separate from the report,
/// so [`observations`] reads only client observations. EDS and Camel write
/// many warnings to stderr, and a warning with an `=` can look like an
/// observation.
struct Run {
    status: std::process::ExitStatus,
    stdout: String,
    report: String,
    /// The number of 401 responses from the mock during the run. Without
    /// this server-side count, the client counts can be correct for a run in
    /// which the token did not become stale.
    unauthorized_responses: usize,
}

/// Waits for the `ready` file of the client, changes the bearer token that
/// the mock accepts (and, in the positive case, the token of the client
/// session), then creates `go`.
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
            // No `go` file. The client stopped, or its handshake fails with a
            // better message than an assertion here can give.
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    // Keep this order. The server first refuses the old token, then the new
    // token becomes available. In the opposite order, the client can fetch
    // the new token first, and the test passes without a 401.
    server.set_bearer_token(FRESH_TOKEN);
    if refresh_serves_fresh_token {
        std::fs::write(token_path, FRESH_TOKEN).expect("write the refreshed access token");
    }

    std::fs::write(go_path, "go").expect("write the harness's go file");
}

/// The positive case: the stored refresh token operates. The operation must
/// complete.
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

    // Assert this first. `uses_oauth2` reads this field. If the keyfile
    // `[Authentication] Method` does not get to it, all observations below
    // are about a password account.
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

    // The connect fetched exactly one token from a real `CamelSession`.
    assert_eq!(
        seen.get("token-fetches-before-rotation"),
        Some(&"1"),
        "the connect did not fetch exactly one access token through the session\n{report}"
    );
    // Exactly one: the silent attempt of the connect through the named
    // mechanism. A second call means that the session rejected the attempt
    // and started its interactive loop. Then the count after the rotation has
    // no meaning.
    assert_eq!(
        seen.get("authenticate-calls-before-rotation"),
        Some(&"1"),
        "the connect's silent attempt through the named mechanism never reached the \
         session, or reached it more than once\n{report}"
    );
    assert_eq!(
        seen.get("folders"),
        Some(&"Inbox,Sent"),
        "the first listing is not the mock's two mailboxes\n{report}"
    );

    // The main assertion. The server refused the token of the open
    // connection, and the user sees nothing.
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

    // How the retry succeeded. One more token fetch is a refresh. A call
    // *after* the call before the rotation is a second credentials request.
    // In the application, that is the hourly re-consent window.
    assert_eq!(
        seen.get("token-fetches"),
        Some(&"2"),
        "the retry did not take exactly one refresh\n{report}"
    );
    assert_eq!(
        seen.get("authenticate-calls"),
        Some(&"1"),
        "the 401 reached the session's interactive authentication a second time: in the \
         running application that is the hourly re-consent window this test exists to avoid\n{report}"
    );

    // The server refused exactly one request. Without this assertion, a run
    // with no stale token gives the same counts, and the test proves nothing.
    assert_eq!(
        unauthorized_responses, 1,
        "the mock refused {unauthorized_responses} requests, not the single stale-token one\n{report}"
    );
}

/// The negative case: the server also refuses the refreshed token (a revoked
/// account, or an expired refresh token). The operation must fail after
/// exactly one refresh. A retry loop shows the re-consent window many times.
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

    // Two refusals: the listing and its one retry. A retry loop increases
    // this count. The client counters cannot tell a second retry from a
    // second token fetch.
    assert_eq!(
        unauthorized_responses, 2,
        "the mock refused {unauthorized_responses} requests, not the attempt and its single retry\n{report}"
    );

    // The user gets an error message. The tests of `jmap-mail` examine its
    // text.
    assert!(
        seen.get("second-listing-error")
            .is_some_and(|message| !message.is_empty()),
        "the failed listing reported no error message at all\n{report}"
    );

    // The escalation is *not* in the failed operation. The store gives the
    // 401 to the caller of the folder tree and stops. No call goes to the
    // session after the silent attempt before the rotation. Thus the prompter
    // occurs at most once for each reconnect, not once for each failed
    // operation. This assertion finds a change of that behaviour.
    assert_eq!(
        seen.get("authenticate-calls"),
        Some(&"1"),
        "the failing operation went back to the session for credentials a second time \
         instead of reporting the failure to its caller\n{report}"
    );
}
