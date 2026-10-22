// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `Email/get` object padding on standalone `jmap-mockd`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

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

    /// Send one API request and return the JSON response body.
    fn api(&self, body: Value) -> Value {
        let address = self
            .origin
            .strip_prefix("http://")
            .expect("the mock serves plain HTTP");
        let mut stream = TcpStream::connect(address).expect("connect to the mock");

        let payload = serde_json::to_vec(&body).expect("serialize API request");
        let headers = format!(
            "POST /jmap HTTP/1.1\r\n\
             Host: {address}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n",
            payload.len()
        );
        stream.write_all(headers.as_bytes()).expect("write headers");
        stream.write_all(&payload).expect("write body");
        stream.flush().expect("flush request");

        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read the response");

        let (head, body) = response
            .split_once("\r\n\r\n")
            .expect("a response with a header block");
        assert!(head.contains("200"), "{head}");
        serde_json::from_str(body).expect("a response JSON body")
    }
}

impl Drop for Mockd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn email_get(mockd: &Mockd, ids: &[&str]) -> Value {
    let response = mockd.api(json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        "methodCalls": [[
            "Email/get",
            {
                "accountId": "A1",
                "ids": ids,
                "properties": ["id", "subject"]
            },
            "g1"
        ]],
    }));
    response["methodResponses"][0][1].clone()
}

fn email_ids(method_response: &Value) -> Vec<String> {
    method_response["list"]
        .as_array()
        .expect("list array")
        .iter()
        .map(|value| {
            value["id"]
                .as_str()
                .expect("email id is a string")
                .to_owned()
        })
        .collect()
}

#[test]
fn the_flag_makes_email_get_include_unrequested_objects() {
    let mockd = Mockd::spawn(&["--email-get-includes-unrequested"]);

    let response = email_get(&mockd, &["E1"]);

    assert_eq!(email_ids(&response), vec!["E1", "E2"]);
}

#[test]
fn help_lists_the_email_get_includes_unrequested_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(
        stderr.contains("--email-get-includes-unrequested"),
        "{stderr}"
    );
}
