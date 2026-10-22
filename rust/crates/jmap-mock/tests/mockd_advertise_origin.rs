// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify advertised session endpoint flags on standalone `jmap-mockd`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

/// A `jmap-mockd` child process. Drop stops it.
struct Mockd {
    child: Child,
    origin: String,
}

impl Mockd {
    /// Spawn `jmap-mockd --port 0 <extra_args>` and wait for its origin.
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

        std::thread::spawn(move || {
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });

        Self { child, origin }
    }

    /// Send one request and return `(status_line, headers, body)`.
    fn request(&self, method: &str, path: &str) -> (String, Vec<String>, String) {
        let address = self
            .origin
            .strip_prefix("http://")
            .expect("the mock serves plain HTTP");
        let mut stream = TcpStream::connect(address).expect("connect to the mock");

        let head = format!(
            "{method} {path} HTTP/1.1\r\n\
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
}

impl Drop for Mockd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_flag_rewrites_session_endpoints_to_the_given_origin() {
    let advertised = "http://plain-http.example";
    let mockd = Mockd::spawn(&["--advertise-origin", advertised]);

    let (status, _, body) = mockd.request("GET", "/.well-known/jmap");

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
fn help_lists_the_advertise_origin_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--advertise-origin"), "{stderr}");
}
