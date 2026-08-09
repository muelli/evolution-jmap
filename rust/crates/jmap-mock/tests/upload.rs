// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the mock does with an upload that is larger than the `maxSizeUpload`
//! it advertises (RFC 8620 §6.1).
//!
//! Asked over a socket rather than through `jmap-client`, deliberately: the
//! client refuses such an upload locally, so a test that went through it could
//! never reach this code. The question here is what a client that *ignores* the
//! session document is told — which is the whole reason the mock enforces the
//! limit it advertises, rather than advertising a number and taking anything.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;

/// Posts `body` to `/upload/<account>` and answers (status line, body) —
/// a whole HTTP/1.1 request written out, because the point is to be a client
/// that does not know what this one knows.
fn post_upload(server: &MockServer, body: &[u8]) -> (String, String) {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");

    let head = format!(
        "POST /upload/{account} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Content-Type: message/rfc822\r\n\
         Content-Length: {length}\r\n\
         Connection: close\r\n\
         \r\n",
        account = server.account_id().as_str(),
        length = body.len(),
    );
    stream.write_all(head.as_bytes()).expect("write headers");
    stream.write_all(body).expect("write body");
    stream.flush().expect("flush the request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the response");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("a response with a header block");
    let status = head.lines().next().expect("a status line").to_owned();
    (status, body.to_owned())
}

#[test]
fn an_upload_over_the_advertised_limit_is_refused_with_the_limit_it_broke() {
    let server = MockServer::builder().size_upload(1024).start();

    let (status, body) = post_upload(&server, &vec![b'x'; 1025]);

    assert!(status.contains("400"), "{status}");
    let problem: serde_json::Value = serde_json::from_str(&body).expect("problem details JSON");
    assert_eq!(problem["type"], "urn:ietf:params:jmap:error:limit");
    // RFC 8620 §3.6.1: the `limit` property names *which* limit was broken.
    // Without it a client cannot tell this from a request that was too large.
    assert_eq!(problem["limit"], "maxSizeUpload");

    // And nothing was stored: a refusal, not a truncation.
    let state = server.state();
    let state = state.lock().unwrap();
    assert!(
        state
            .account(&server.account_id())
            .unwrap()
            .blobs
            .is_empty()
    );
}

#[test]
fn an_upload_of_exactly_the_advertised_limit_is_taken() {
    let server = MockServer::builder().size_upload(1024).start();

    let (status, _) = post_upload(&server, &vec![b'x'; 1024]);

    assert!(status.contains("201"), "{status}");
    let state = server.state();
    let state = state.lock().unwrap();
    assert_eq!(
        state.account(&server.account_id()).unwrap().blobs.len(),
        1,
        "an upload of exactly the limit was not stored"
    );
}
