// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Item 101(b)'s acceptance test, made headless: the endless OAuth2 consent
//! loop the operator hit on 0.5.0 came from a 401 that only arrived after a
//! redirect to a different origin had dropped
//! the `Authorization` header, treated as a rejected credential and
//! escalated to a fresh interactive consent — forever, since the fresh
//! token hit the same redirect and the same 401 every time.
//!
//! `jmap_client::Error::RedirectCredentialsWithheld`
//! (`jmap-client/src/client.rs`'s `execute_within`) already keeps that case
//! out of `is_wrong_password()`, and a unit test at that layer already
//! covers it; what had never run was a real `EBookMetaBackend` connect
//! through the scenario, against a real `evolution-source-registry` and a
//! real `evolution-addressbook-factory`.
//!
//! The fixture needs a redirect to a genuinely different *origin*, not just
//! a different path: `jmap_client::url::same_origin` differs on port alone,
//! so two loopback mocks on two different ports already qualify, with no
//! need to involve a second hostname. `jmap_mock::MockServer`'s own
//! `session_via_redirect` mode redirects `/.well-known/jmap` to a second
//! path on the *same* origin — built for a different bug (item 61) — so it
//! cannot stand in here; extending it would also cross into `jmap-mock`,
//! which is codex's turf. Instead this test runs its own tiny front door: a
//! raw `TcpListener` that answers every request with a 302 to the real mock
//! on a different port, which requires Basic auth and so 401s the
//! credentials-stripped request that reaches it after the hop.

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

/// Bind a loopback port that answers every request with a 302 to
/// `{target_origin}/.well-known/jmap`, regardless of path, method or
/// headers — this fixture needs no more than that one redirect to make the
/// origin change. Returns the port and a counter of how many requests it
/// answered, so the test can assert there was exactly one: a backend that
/// retried here would mean the redirect-withheld classification had not, in
/// fact, kept this out of a retry loop.
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
            // Only the request line and headers matter, and a GET carries no
            // body; draining a bounded read is enough to let the client see
            // its write complete before this closes the connection.
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
    assert_eq!(
        seen.get("connect-ok"),
        Some(&"0"),
        "the connect through a credentials-dropping redirect should fail, not succeed\n{report}"
    );
    assert_eq!(
        seen.get("credentials-required"),
        Some(&"0"),
        "a redirect-withheld 401 must not escalate to interactive consent — this is exactly \
         the endless OAuth2 popup loop item 101 reports\n{report}"
    );
    let connect_error = seen
        .get("connect-error")
        .unwrap_or_else(|| panic!("no connect-error observation\n{report}"));
    assert!(
        connect_error.contains("redirect") && connect_error.contains("withheld"),
        "expected the redirect-credentials-withheld message, got {connect_error:?}\n{report}"
    );

    assert_eq!(
        front_door_hits.load(Ordering::SeqCst),
        1,
        "the front door should see exactly one request: a backend that retried after the \
         redirect-withheld failure would be the same retry storm item 101 reports\n{report}"
    );
    assert_eq!(
        server.unauthorized_responses(),
        1,
        "the real server should see exactly one credentials-stripped request, not a loop\n{report}"
    );
}
