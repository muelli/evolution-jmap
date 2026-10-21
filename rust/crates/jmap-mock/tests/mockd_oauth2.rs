// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `--oauth2` discovery and registration on standalone `jmap-mockd`.
//! Uses raw `TcpStream` I/O for CLI-only behavior without extra HTTP client dependencies.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

/// A `jmap-mockd` child process. Drop kills it.
struct Mockd {
    child: Child,
    origin: String,
}

impl Mockd {
    /// Spawn `jmap-mockd --port 0 <extra_args>` and wait for its bound origin.
    fn spawn(extra_args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
            .arg("--port")
            .arg("0")
            .args(extra_args)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn jmap-mockd");

        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = BufReader::new(stdout);
        let mut first_line = String::new();
        reader
            .read_line(&mut first_line)
            .expect("a readable startup line");
        let origin = first_line
            .trim_end()
            .strip_prefix("jmap-mockd listening on ")
            .unwrap_or_else(|| panic!("unexpected startup line: {first_line}"))
            .to_owned();

        // Drain stdout so child `println!` calls keep a live pipe.
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });

        Self { child, origin }
    }

    /// Send one HTTP/1.1 request and return `(status_line, body)`.
    fn request(&self, method: &str, path: &str, body: &[u8]) -> (String, String) {
        let address = self
            .origin
            .strip_prefix("http://")
            .expect("the mock serves plain HTTP");
        let mut stream = TcpStream::connect(address).expect("connect to the mock");

        let head = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {address}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {length}\r\n\
             Connection: close\r\n\
             \r\n",
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
}

impl Drop for Mockd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn without_the_flag_the_metadata_endpoint_is_not_published() {
    let mockd = Mockd::spawn(&[]);

    let (status, _) = mockd.request("GET", "/.well-known/oauth-authorization-server", b"");

    assert!(status.contains("404"), "{status}");
}

#[test]
fn the_oauth2_flag_publishes_a_matching_metadata_document() {
    let mockd = Mockd::spawn(&["--oauth2"]);

    let (status, body) = mockd.request("GET", "/.well-known/oauth-authorization-server", b"");

    assert!(status.contains("200"), "{status}");
    let document: serde_json::Value = serde_json::from_str(&body).expect("a JSON document");
    assert_eq!(document["issuer"], mockd.origin);
    assert_eq!(
        document["registration_endpoint"],
        format!("{}/oauth/register", mockd.origin)
    );
    assert_eq!(document["grant_types_supported"][0], "authorization_code");
}

#[test]
fn the_oauth2_flag_also_answers_client_registration() {
    let mockd = Mockd::spawn(&["--oauth2"]);

    let (status, body) = mockd.request("POST", "/oauth/register", br#"{"client_name":"test"}"#);

    assert!(status.contains("201"), "{status}");
    let response: serde_json::Value = serde_json::from_str(&body).expect("a JSON document");
    assert!(
        response["client_id"].is_string(),
        "expected a client_id, got {response}"
    );
}
