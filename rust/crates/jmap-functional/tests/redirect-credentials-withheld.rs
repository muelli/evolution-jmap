// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A headless acceptance test for the endless OAuth2 consent loop of 0.5.0.
//!
//! A redirect to a different origin removed the `Authorization` header. The
//! server then sent a 401, and the backend treated it as a rejected
//! credential. This caused a new interactive consent. The new token got the
//! same redirect and the same 401, so the loop did not stop.
//!
//! `jmap_client::Error::RedirectCredentialsWithheld` (from `execute_within`
//! in `jmap-client/src/client.rs`) keeps this case out of
//! `is_wrong_password()`, and a unit test covers it at that layer. This test
//! does a real `EBookMetaBackend` connect through the scenario, against a
//! real `evolution-source-registry` and `evolution-addressbook-factory`.
//!
//! EDS `ensure_connected_sync` (`e-book-meta-backend.c`) schedules the
//! interactive `credentials-required` consent for each
//! `ESourceAuthenticationResult` other than `UNKNOWN`. It ignores
//! `is_wrong_password`. Thus `jmap_backend_core::connect::ConnectError::auth_result`
//! maps `RedirectCredentialsWithheld` to `UNKNOWN`, not `ERROR`. For `UNKNOWN`,
//! `ensure_connected_sync` also gives the real `GError` and not a false success.
//!
//! The fixture needs a redirect to a different *origin*, not only a different
//! path. `jmap_client::url::same_origin` compares the port, so two loopback
//! mocks on two ports are sufficient. The `session_via_redirect` mode of
//! `jmap_mock::MockServer` redirects to the *same* origin, so it is not
//! applicable here. Thus this test runs its own front door: a raw
//! `TcpListener` that sends a 302 for each request to the real mock on a
//! different port. The mock requires Basic auth, so it sends a 401 for the
//! request without credentials that arrives after the redirect.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jmap_functional::{Session, observations, required_path};

const USER: &str = "jmap-redirect-withheld";
const PASSWORD: &str = "the-stored-password";

fn keyfile(front_door_port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP redirect-credentials-withheld functional test\n\
         Enabled=true\n\
         \n\
         [Address Book]\n\
         BackendName=jmap\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={front_door_port}\n\
         User={USER}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// Binds a loopback port that sends a 302 to
/// `{target_origin}/.well-known/jmap` for each request, for all paths,
/// methods and headers. Returns the port and a counter of the answered
/// requests. The test uses the counter to make sure that the backend does
/// not retry in a loop.
fn spawn_redirecting_front_door(target_origin: String) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the front door's port");
    let port = listener
        .local_addr()
        .expect("the bound address has a port")
        .port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { continue };
            // A GET has no body. One bounded read lets the client complete
            // its write before this closes the connection.
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            counted.fetch_add(1, Ordering::SeqCst);
            let location = format!("{target_origin}/.well-known/jmap");
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (port, hits)
}

#[test]
fn a_cross_origin_redirect_that_drops_credentials_fails_without_escalating() {
    let client = required_path("JMAP_FUNCTIONAL_REDIRECT_CREDENTIALS_WITHHELD_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_BOOK_MODULE");

    let server = jmap_mock::MockServer::builder()
        .basic_auth(USER, PASSWORD)
        .start();

    let (front_door_port, front_door_hits) =
        spawn_redirecting_front_door(server.origin().to_owned());

    const ACCOUNT_UID: &str = "jmap-functional-redirect-credentials-withheld";
    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/redirect-credentials-withheld"
    ));
    session.write_source(ACCOUNT_UID, &keyfile(front_door_port));
    session.stage_address_book_backend(&module);

    let output = session.run(&client, &[ACCOUNT_UID, PASSWORD]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );
    // The first connect runs before EDS reads the secret store (see the doc
    // of `jmap_backend_core::connect::credentials`). Thus it always fails with
    // `CredentialsRequired` and schedules the one expected consent request.
    // This is the start condition of the fixture, not the bug under test.
    // EDS then retries with the stored password. This is the first of the two
    // redirect-withheld network attempts.
    assert_eq!(
        seen.get("credentials-required-after-connect"),
        Some(&"1"),
        "the first connect has no password to try yet and should ask for one exactly once\n\
         {report}"
    );
    // The prompter answers that request with the password. This causes the
    // second redirect-withheld attempt. The answer schedules one more request
    // (EDS registry records the new authentication). The 401 of an attempt
    // must not add a request. Such a repeat is the endless OAuth2 popup loop.
    assert_eq!(
        seen.get("credentials-required-after-reauthenticate"),
        Some(&"2"),
        "answering the one expected consent request should schedule at most one more, not an \
         escalation from the redirect-withheld 401 itself\n{report}"
    );
    assert_eq!(
        seen.get("credentials-required"),
        Some(&"2"),
        "with nothing here asking again, the count must not keep climbing on its own: that is \
         exactly the endless OAuth2 popup loop this test exists to catch\n{report}"
    );

    assert_eq!(
        front_door_hits.load(Ordering::SeqCst),
        2,
        "two independent attempts reach the network (EDS's own retry once the password is \
         stored, then the explicit reauthenticate); a backend that retried within either \
         would be the same retry storm this test exists to catch\n{report}"
    );
    assert_eq!(
        server.unauthorized_responses(),
        2,
        "the real server should see exactly one credentials-stripped request per attempt, \
         not a loop\n{report}"
    );
}
