// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile discovery redirects for `/.well-known/jmap`.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;

/// Send one GET request and return `(status_line, headers, body)`.
fn get(server: &MockServer, path: &str) -> (String, Vec<String>, String) {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");

    let head = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(head.as_bytes()).expect("write headers");
    stream.flush().expect("flush the request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the response");

    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("a response with a header block");
    let mut lines = head.lines();
    let status = lines.next().expect("a status line").to_owned();
    let headers = lines.map(str::to_owned).collect();
    (status, headers, body.to_owned())
}

fn header_value<'a>(headers: &'a [String], name: &str) -> Option<&'a str> {
    headers.iter().find_map(|header| {
        let (key, value) = header.split_once(':')?;
        if key.eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
}

#[test]
fn well_known_redirect_to_origin_sends_307_with_a_location_header() {
    let server = MockServer::builder()
        .well_known_redirect_to("http://foreign.example")
        .start();

    let (status, headers, _) = get(&server, "/.well-known/jmap");

    assert!(status.contains("307"), "{status}");
    assert_eq!(
        header_value(&headers, "Location"),
        Some("http://foreign.example/jmap/session")
    );
}

#[test]
fn without_the_flag_the_well_known_route_still_serves_a_session_document() {
    let server = MockServer::builder().start();

    let (status, _, body) = get(&server, "/.well-known/jmap");

    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["apiUrl"], format!("{}/jmap", server.origin()));
}
