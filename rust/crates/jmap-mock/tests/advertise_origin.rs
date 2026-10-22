// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify advertised session endpoints on the in-process mock server.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;

/// Send one request and return `(status_line, headers, body)`.
fn get(server: &MockServer, path: &str) -> (String, Vec<String>, String) {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock server serves plain HTTP");
    let mut stream = TcpStream::connect(address).expect("connect to the mock server");

    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream
        .write_all(request.as_bytes())
        .expect("write request bytes");
    stream.flush().expect("flush request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read response bytes");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("response has a header block");
    let mut lines = head.lines();
    let status = lines.next().expect("response has status line").to_owned();
    let headers = lines.map(str::to_owned).collect();
    (status, headers, body.to_owned())
}

#[test]
fn advertise_origin_rewrites_session_endpoints() {
    let advertised = "http://plain-http.example";
    let server = MockServer::builder().advertise_origin(advertised).start();

    let (status, _, body) = get(&server, "/.well-known/jmap");

    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["apiUrl"], format!("{advertised}/jmap"));
    assert_eq!(
        session["downloadUrl"],
        format!("{advertised}/download/{{accountId}}/{{blobId}}/{{name}}")
    );
    assert_eq!(
        session["uploadUrl"],
        format!("{advertised}/upload/{{accountId}}")
    );
    assert_eq!(
        session["eventSourceUrl"],
        format!("{advertised}/eventsource")
    );
}

#[test]
fn without_advertise_origin_the_mock_uses_its_own_origin() {
    let server = MockServer::builder().start();

    let (status, _, body) = get(&server, "/.well-known/jmap");

    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["apiUrl"], format!("{}/jmap", server.origin()));
}
