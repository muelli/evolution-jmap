// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A token endpoint that answers with a cross-origin 307 or 308.
//!
//! A 307 or 308 tells the client to send the same method and the same body
//! again to the new location (RFC 9110 Section 15.4.8 and 15.4.9). The body of
//! a token request holds the refresh token, or the code and the PKCE verifier.
//! The transport removes `Authorization` on a cross-origin hop, but a public
//! client sends its secrets in the body. Thus the transport must not send a
//! POST body to an origin that the caller did not name.
//! Cross-protocol audit (ROADMAP 169), Area 8, row X8-2.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use jmap_client::oauth::{self, PkceVerifier};
use jmap_client::transport::UreqTransport;

/// Answers `127.0.0.1` with a redirect to `localhost` (another origin, the
/// same port), and records whether a request ever arrived for `localhost`.
struct RedirectingTokenEndpoint {
    port: u16,
    foreign_hit: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RedirectingTokenEndpoint {
    fn start(status: u16) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind token endpoint");
        let port = server.server_addr().to_ip().expect("server has ip").port();
        let foreign_hit = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let foreign_hit = Arc::clone(&foreign_hit);
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(20)) {
                        Ok(Some(request)) => {
                            let host = request
                                .headers()
                                .iter()
                                .find(|h| h.field.equiv("Host"))
                                .map(|h| h.value.as_str().to_owned())
                                .unwrap_or_default();
                            if host.starts_with("localhost") {
                                foreign_hit.store(true, Ordering::SeqCst);
                                let body = br#"{"access_token":"t","token_type":"bearer"}"#;
                                let response = tiny_http::Response::from_data(&body[..])
                                    .with_header(
                                        tiny_http::Header::from_bytes(
                                            &b"Content-Type"[..],
                                            &b"application/json"[..],
                                        )
                                        .expect("content type header"),
                                    );
                                let _ = request.respond(response);
                            } else {
                                let location = format!("http://localhost:{port}/token");
                                let response = tiny_http::Response::empty(status).with_header(
                                    tiny_http::Header::from_bytes(
                                        &b"Location"[..],
                                        location.as_bytes(),
                                    )
                                    .expect("location header"),
                                );
                                let _ = request.respond(response);
                            }
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            }
        });
        Self {
            port,
            foreign_hit,
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/token", self.port)
    }
}

impl Drop for RedirectingTokenEndpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn a_cross_origin_307_or_308_never_resends_the_refresh_token() {
    for status in [307, 308] {
        let endpoint = RedirectingTokenEndpoint::start(status);
        let result = oauth::refresh_access_token(
            &UreqTransport::default(),
            &endpoint.url(),
            "client",
            "refresh-secret",
            None,
        );
        assert!(
            !endpoint.foreign_hit.load(Ordering::SeqCst),
            "a {status} sent the refresh token to another origin"
        );
        assert!(result.is_err(), "a {status} to another origin must fail");
    }
}

#[test]
fn a_cross_origin_307_never_resends_the_code_and_verifier() {
    let endpoint = RedirectingTokenEndpoint::start(307);
    let result = oauth::exchange_code(
        &UreqTransport::default(),
        &endpoint.url(),
        "client",
        "code",
        "https://client.example.org/cb",
        &PkceVerifier::generate(),
        None,
    );
    assert!(
        !endpoint.foreign_hit.load(Ordering::SeqCst),
        "a 307 sent the code and the PKCE verifier to another origin"
    );
    assert!(result.is_err());
}
