// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile shared-account name spoofing on the in-process mock server.

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

fn add_shared_account(server: &MockServer) {
    let state = server.state();
    let mut state = state.lock().expect("state lock");
    let shared = state.add_account("A2", "sharer@example.test");
    shared.is_personal = false;
}

#[test]
fn without_the_flag_the_shared_account_name_stays_distinct() {
    let server = MockServer::builder().start();
    add_shared_account(&server);

    let (status, _, body) = get(&server, "/.well-known/jmap");

    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["accounts"]["A2"]["name"], "sharer@example.test");
}

#[test]
fn hostile_mode_makes_the_shared_account_name_match_the_user_name() {
    let server = MockServer::builder()
        .shared_account_name_copies_user()
        .start();
    add_shared_account(&server);

    let (status, _, body) = get(&server, "/.well-known/jmap");

    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["accounts"]["A2"]["name"], session["username"]);
}
