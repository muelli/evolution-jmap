// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile calendar-event link URI flags on standalone `jmap-mockd`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

const SAFE_LINK_URI: &str = "https://files.example.com/agenda.pdf";
const HOSTILE_LINK_URI: &str = "file:///dev/zero";

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

fn default_calendar_id(mockd: &Mockd) -> String {
    let response = mockd.api(json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
        "methodCalls": [[
            "Calendar/get",
            {"accountId": "A1", "properties": ["id"]},
            "c1"
        ]],
    }));

    response["methodResponses"][0][1]["list"][0]["id"]
        .as_str()
        .expect("default calendar id")
        .to_owned()
}

fn create_event_with_link(mockd: &Mockd) -> String {
    let calendar_id = default_calendar_id(mockd);
    let mut calendar_ids = serde_json::Map::new();
    calendar_ids.insert(calendar_id, Value::Bool(true));

    let response = mockd.api(json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
        "methodCalls": [[
            "CalendarEvent/set",
            {
                "accountId": "A1",
                "create": {
                    "k1": {
                        "@type": "Event",
                        "version": "2.0",
                        "calendarIds": calendar_ids,
                        "start": "2026-01-02T12:00:00",
                        "duration": "PT1H",
                        "title": "Planning",
                        "links": {
                            "l1": {
                                "@type": "Link",
                                "href": SAFE_LINK_URI,
                                "title": "Agenda"
                            }
                        }
                    }
                }
            },
            "s1"
        ]],
    }));

    response["methodResponses"][0][1]["created"]["k1"]["id"]
        .as_str()
        .expect("created event id")
        .to_owned()
}

fn event_link_uri(mockd: &Mockd, event_id: &str) -> String {
    let response = mockd.api(json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
        "methodCalls": [[
            "CalendarEvent/get",
            {
                "accountId": "A1",
                "ids": [event_id],
                "properties": ["id", "links"]
            },
            "g1"
        ]],
    }));

    response["methodResponses"][0][1]["list"][0]["links"]["l1"]["href"]
        .as_str()
        .expect("link URI as string")
        .to_owned()
}

#[test]
fn the_flag_rewrites_calendar_event_link_uri_to_file_scheme() {
    let mockd = Mockd::spawn(&["--calendar-event-link-uses-file-uri"]);
    let event_id = create_event_with_link(&mockd);

    let uri = event_link_uri(&mockd, &event_id);

    assert_eq!(uri, HOSTILE_LINK_URI);
}

#[test]
fn help_lists_the_calendar_event_link_uses_file_uri_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(
        stderr.contains("--calendar-event-link-uses-file-uri"),
        "{stderr}"
    );
}
