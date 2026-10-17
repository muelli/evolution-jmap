// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mutation-test coverage suite for jmap-client/src/transport.rs.
//!
//! Pins CancelFlag/CancelScope lifecycles, TransportError formatting,
//! UreqTransport timeouts, redirect loops, origin-sensitive header stripping,
//! POST-to-GET method rewriting, and response size ceilings.

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use jmap_client::transport::{
    CancelFlag, CancelScope, HttpMethod, HttpRequest, Transport, TransportError, UreqTransport,
    observed,
};

#[test]
fn cancel_flag_lifecycle_and_state() {
    let flag = CancelFlag::new();
    assert!(!flag.is_cancelled());

    flag.cancel();
    assert!(flag.is_cancelled());

    let default_flag = CancelFlag::default();
    assert!(!default_flag.is_cancelled());

    let clone = flag.clone();
    assert!(clone.is_cancelled());
}

#[test]
fn cancel_scope_nesting_and_observation() {
    assert!(observed().is_none());

    let outer_flag = CancelFlag::new();
    outer_flag.cancel();

    {
        let scope1 = CancelScope::install(&outer_flag);
        let obs1 = observed().expect("outer scope must be observed");
        assert!(obs1.is_cancelled());

        {
            let inner_flag = CancelFlag::new();
            let _scope2 = CancelScope::install(&inner_flag);
            let obs2 = observed().expect("inner scope must be observed");
            assert!(!obs2.is_cancelled());
        }

        let obs1_restored = observed().expect("outer scope must be restored after inner drop");
        assert!(obs1_restored.is_cancelled());

        drop(scope1);
    }

    assert!(observed().is_none());
}

#[test]
fn transport_error_display_formatting_and_error_trait() {
    let cancelled = TransportError::Cancelled;
    assert_eq!(cancelled.to_string(), "transport request cancelled");
    assert!(std::error::Error::source(&cancelled).is_none());

    let failed = TransportError::Failed("connection refused".to_owned());
    assert_eq!(failed.to_string(), "transport error: connection refused");
    assert!(std::error::Error::source(&failed).is_none());

    let too_large = TransportError::ResponseTooLarge { limit: 16384 };
    assert_eq!(
        too_large.to_string(),
        "response body exceeded maximum limit of 16384 bytes"
    );
    assert!(std::error::Error::source(&too_large).is_none());
}

#[test]
fn ureq_transport_executes_cancellation_check_before_request() {
    let cancelled_flag = CancelFlag::new();
    cancelled_flag.cancel();

    let transport = UreqTransport::default();
    let request = HttpRequest {
        method: HttpMethod::Get,
        url: "http://127.0.0.1:1/unreachable",
        headers: &[],
        body: None,
        cancel: Some(&cancelled_flag),
        max_response_bytes: 1024,
    };

    let result = transport.execute(request);
    assert!(matches!(result, Err(TransportError::Cancelled)));
}

#[test]
fn ureq_transport_with_timeouts_enforces_bounded_wait_on_hung_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 128];
            let _ = stream.read(&mut buf);
            while !stop_thread.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(20));
            }
        }
    });

    let transport = UreqTransport::with_timeouts(
        Duration::from_millis(50),
        Duration::from_millis(50),
        Duration::from_millis(100),
    );

    let start = std::time::Instant::now();
    let result = transport.execute(HttpRequest {
        method: HttpMethod::Get,
        url: &format!("http://127.0.0.1:{port}/hung"),
        headers: &[],
        body: None,
        cancel: None,
        max_response_bytes: 1024,
    });
    let elapsed = start.elapsed();

    stop.store(true, Ordering::SeqCst);
    let _ = handle.join();

    assert!(result.is_err(), "hung server must produce transport error");
    assert!(
        elapsed < Duration::from_secs(5),
        "timeout must be enforced quickly, elapsed: {elapsed:?}"
    );
}

#[test]
fn redirect_loop_bounded_to_ten_hops() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind server");
    let port = server.server_addr().to_ip().expect("ip").port();
    let base_url = format!("http://127.0.0.1:{port}");

    let count = Arc::new(AtomicUsize::new(0));
    let count_thread = Arc::clone(&count);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_thread.load(Ordering::SeqCst) {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) {
                let n = count_thread.fetch_add(1, Ordering::SeqCst);
                if n >= 20 {
                    let _ = request.respond(tiny_http::Response::empty(500));
                } else {
                    let response = tiny_http::Response::empty(302).with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/loop"[..])
                            .expect("header"),
                    );
                    let _ = request.respond(response);
                }
            }
        }
    });

    let transport = UreqTransport::new(Duration::from_secs(2));
    let result = transport.execute(HttpRequest {
        method: HttpMethod::Get,
        url: &format!("{base_url}/loop"),
        headers: &[],
        body: None,
        cancel: None,
        max_response_bytes: 1024,
    });

    stop.store(true, Ordering::SeqCst);
    let _ = handle.join();

    match result {
        Err(TransportError::Failed(msg)) => {
            assert!(
                msg.contains("too many redirects"),
                "expected 'too many redirects', got: {msg}"
            );
        }
        Ok(res) => panic!(
            "expected too many redirects error, got status {}",
            res.status
        ),
        Err(err) => panic!("expected Failed with too many redirects, got: {err}"),
    }

    assert_eq!(
        count.load(Ordering::SeqCst),
        11,
        "redirect loop must be capped at exactly 10 redirects (11 requests total)"
    );
}

#[test]
fn redirect_status_without_location_header_does_not_redirect() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind server");
    let port = server.server_addr().to_ip().expect("ip").port();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_thread.load(Ordering::SeqCst) {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) {
                let response =
                    tiny_http::Response::from_string("no location").with_status_code(302);
                let _ = request.respond(response);
            }
        }
    });

    let transport = UreqTransport::new(Duration::from_secs(2));
    let result = transport.execute(HttpRequest {
        method: HttpMethod::Get,
        url: &format!("http://127.0.0.1:{port}/no-location"),
        headers: &[],
        body: None,
        cancel: None,
        max_response_bytes: 1024,
    });

    stop.store(true, Ordering::SeqCst);
    let _ = handle.join();

    let response = result.expect("response without location header must be returned as data");
    assert_eq!(response.status, 302);
    assert_eq!(String::from_utf8_lossy(&response.body), "no location");
}

#[test]
fn cross_origin_and_same_origin_redirect_authorization_handling() {
    let server_a = tiny_http::Server::http("127.0.0.1:0").expect("bind server a");
    let port_a = server_a.server_addr().to_ip().expect("ip a").port();

    let server_b = tiny_http::Server::http("127.0.0.2:0").expect("bind server b");
    let port_b = server_b.server_addr().to_ip().expect("ip b").port();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_a = Arc::clone(&stop);
    let handle_a = thread::spawn(move || {
        while !stop_a.load(Ordering::SeqCst) {
            if let Ok(Some(request)) = server_a.recv_timeout(Duration::from_millis(20)) {
                let url = request.url().to_owned();
                if url == "/redir-same" {
                    let response = tiny_http::Response::empty(302).with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/same-dest"[..])
                            .expect("location"),
                    );
                    let _ = request.respond(response);
                } else if url == "/same-dest" {
                    let has_auth = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("authorization"));
                    let has_custom = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("x-custom-test"));
                    let body = format!("same_dest:auth={has_auth},custom={has_custom}");
                    let response = tiny_http::Response::from_string(body);
                    let _ = request.respond(response);
                } else if url == "/redir-cross" {
                    let target = format!("http://127.0.0.2:{port_b}/cross-dest");
                    let response = tiny_http::Response::empty(302).with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], target.as_bytes())
                            .expect("location"),
                    );
                    let _ = request.respond(response);
                } else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }
        }
    });

    let stop_b = Arc::clone(&stop);
    let handle_b = thread::spawn(move || {
        while !stop_b.load(Ordering::SeqCst) {
            if let Ok(Some(request)) = server_b.recv_timeout(Duration::from_millis(20)) {
                let has_auth = request
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("authorization"));
                let has_custom = request
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("x-custom-test"));
                let body = format!("cross_dest:auth={has_auth},custom={has_custom}");
                let response = tiny_http::Response::from_string(body);
                let _ = request.respond(response);
            }
        }
    });

    let transport = UreqTransport::new(Duration::from_secs(5));
    let headers = vec![
        ("Authorization".to_owned(), "Bearer secret-token".to_owned()),
        ("X-Custom-Test".to_owned(), "present".to_owned()),
    ];

    // Same-origin redirect must preserve Authorization and custom headers.
    let res_same = transport
        .execute(HttpRequest {
            method: HttpMethod::Get,
            url: &format!("http://127.0.0.1:{port_a}/redir-same"),
            headers: &headers,
            body: None,
            cancel: None,
            max_response_bytes: 1024,
        })
        .expect("same origin request succeeds");
    let same_body = String::from_utf8_lossy(&res_same.body);
    assert_eq!(same_body, "same_dest:auth=true,custom=true");

    // Cross-origin redirect must strip Authorization while preserving custom headers.
    let res_cross = transport
        .execute(HttpRequest {
            method: HttpMethod::Get,
            url: &format!("http://127.0.0.1:{port_a}/redir-cross"),
            headers: &headers,
            body: None,
            cancel: None,
            max_response_bytes: 1024,
        })
        .expect("cross origin request succeeds");
    let cross_body = String::from_utf8_lossy(&res_cross.body);
    assert_eq!(cross_body, "cross_dest:auth=false,custom=true");

    stop.store(true, Ordering::SeqCst);
    let _ = handle_a.join();
    let _ = handle_b.join();
}

#[test]
fn post_redirect_rewriting_to_get_and_header_cleanup() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind server");
    let port = server.server_addr().to_ip().expect("ip").port();
    let base_url = format!("http://127.0.0.1:{port}");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_thread.load(Ordering::SeqCst) {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) {
                let url = request.url().to_owned();
                let method = request.method().as_str().to_owned();
                if url == "/post-303" {
                    let response = tiny_http::Response::empty(303).with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/target-303"[..])
                            .expect("location"),
                    );
                    let _ = request.respond(response);
                } else if url == "/target-303" {
                    let has_ct = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("content-type"));
                    let has_cl = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("content-length"));
                    let has_custom = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("x-keep-custom"));
                    let body = format!(
                        "target_303:method={method},ct={has_ct},cl={has_cl},custom={has_custom}"
                    );
                    let response = tiny_http::Response::from_string(body);
                    let _ = request.respond(response);
                } else if url == "/get-302" {
                    let response = tiny_http::Response::empty(302).with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/target-get-302"[..])
                            .expect("location"),
                    );
                    let _ = request.respond(response);
                } else if url == "/target-get-302" {
                    let has_ct = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("content-type"));
                    let body = format!("target_get:method={method},ct={has_ct}");
                    let response = tiny_http::Response::from_string(body);
                    let _ = request.respond(response);
                } else if url == "/post-307" {
                    let response = tiny_http::Response::empty(307).with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/target-307"[..])
                            .expect("location"),
                    );
                    let _ = request.respond(response);
                } else if url == "/target-307" {
                    let has_ct = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("content-type"));
                    let body = format!("target_307:method={method},ct={has_ct}");
                    let response = tiny_http::Response::from_string(body);
                    let _ = request.respond(response);
                } else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }
        }
    });

    let transport = UreqTransport::with_timeouts(
        Duration::from_millis(300),
        Duration::from_millis(300),
        Duration::from_millis(800),
    );

    // 1. POST receiving 303 must switch method to GET and drop Content-Type and Content-Length.
    let post_headers = vec![
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Content-Length".to_owned(), "0".to_owned()),
        ("X-Keep-Custom".to_owned(), "preserved".to_owned()),
    ];
    let res_303 = transport
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: &format!("{base_url}/post-303"),
            headers: &post_headers,
            body: Some(b""),
            cancel: None,
            max_response_bytes: 1024,
        })
        .expect("303 redirect succeeds");
    let body_303 = String::from_utf8_lossy(&res_303.body);
    assert_eq!(
        body_303,
        "target_303:method=GET,ct=false,cl=false,custom=true"
    );

    // 2. GET receiving 302 must stay GET and must NOT strip Content-Type.
    let get_headers = vec![("Content-Type".to_owned(), "text/plain".to_owned())];
    let res_get = transport
        .execute(HttpRequest {
            method: HttpMethod::Get,
            url: &format!("{base_url}/get-302"),
            headers: &get_headers,
            body: None,
            cancel: None,
            max_response_bytes: 1024,
        })
        .expect("302 get redirect succeeds");
    let body_get = String::from_utf8_lossy(&res_get.body);
    assert_eq!(body_get, "target_get:method=GET,ct=true");

    // 3. POST receiving 307 must preserve POST method and Content-Type per RFC 7231 Section 6.4.7.
    let res_307 = transport
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: &format!("{base_url}/post-307"),
            headers: &post_headers,
            body: Some(b"{\"hello\":true}"),
            cancel: None,
            max_response_bytes: 1024,
        })
        .expect("307 post redirect succeeds");
    let body_307 = String::from_utf8_lossy(&res_307.body);
    assert_eq!(body_307, "target_307:method=POST,ct=true");

    stop.store(true, Ordering::SeqCst);
    let _ = handle.join();
}

#[test]
fn response_body_limit_enforcement() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind server");
    let port = server.server_addr().to_ip().expect("ip").port();
    let base_url = format!("http://127.0.0.1:{port}");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_thread.load(Ordering::SeqCst) {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) {
                let body = b"0123456789"; // 10 bytes
                let response = tiny_http::Response::from_data(body.to_vec()).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/plain"[..])
                        .expect("content type"),
                );
                let _ = request.respond(response);
            }
        }
    });

    let transport = UreqTransport::new(Duration::from_secs(5));

    // Request with max_response_bytes: 5 must fail with ResponseTooLarge { limit: 5 }.
    let result_too_small = transport.execute(HttpRequest {
        method: HttpMethod::Get,
        url: &format!("{base_url}/data"),
        headers: &[],
        body: None,
        cancel: None,
        max_response_bytes: 5,
    });
    match result_too_small {
        Err(TransportError::ResponseTooLarge { limit }) => assert_eq!(limit, 5),
        Ok(res) => panic!("expected ResponseTooLarge, got status {}", res.status),
        Err(err) => panic!("expected ResponseTooLarge, got: {err}"),
    }

    // Request with max_response_bytes: 10 must succeed and return complete body.
    let result_exact = transport
        .execute(HttpRequest {
            method: HttpMethod::Get,
            url: &format!("{base_url}/data"),
            headers: &[],
            body: None,
            cancel: None,
            max_response_bytes: 10,
        })
        .expect("exact body limit succeeds");
    assert_eq!(result_exact.body, b"0123456789");
    assert_eq!(result_exact.status, 200);
    assert_eq!(result_exact.content_type.as_deref(), Some("text/plain"));
    assert_eq!(result_exact.final_url, format!("{base_url}/data"));

    stop.store(true, Ordering::SeqCst);
    let _ = handle.join();
}
