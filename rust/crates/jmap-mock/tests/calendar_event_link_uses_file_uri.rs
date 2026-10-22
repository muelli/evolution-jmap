// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `CalendarEvent/get` link URIs on the in-process mock server.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use jmap_proto::calendars::CalendarEvent;
use serde_json::{Value, json};

const SAFE_LINK_URI: &str = "https://files.example.com/agenda.pdf";
const HOSTILE_LINK_URI: &str = "file:///dev/zero";

/// Send one API request and return the JSON response body.
fn api(server: &MockServer, body: Value) -> Value {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");

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

fn seed_calendar_event_with_link(server: &MockServer) -> String {
    let state = server.state();
    let mut state = state.lock().expect("state lock");
    let account = state
        .account_mut(&server.account_id())
        .expect("default account exists");

    let event_id = account.calendar_events.alloc_id();
    let event = CalendarEvent {
        id: Some(event_id.clone()),
        links: Some(BTreeMap::from([(
            "l1".to_owned(),
            json!({
                "@type": "Link",
                "href": SAFE_LINK_URI,
                "title": "Agenda",
            }),
        )])),
        ..CalendarEvent::default()
    };
    account
        .calendar_events
        .seed_with_id(event_id.clone(), event);

    event_id.to_string()
}

fn event_link_uri(server: &MockServer, event_id: &str) -> String {
    let response = api(
        server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
            "methodCalls": [[
                "CalendarEvent/get",
                {
                    "accountId": "A1",
                    "ids": [event_id],
                    "properties": ["id", "links"]
                },
                "e1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["links"]["l1"]["href"]
        .as_str()
        .expect("link URI as string")
        .to_owned()
}

#[test]
fn without_the_flag_calendar_event_link_uri_stays_unchanged() {
    let server = MockServer::builder().start();
    let event_id = seed_calendar_event_with_link(&server);

    let uri = event_link_uri(&server, &event_id);

    assert_eq!(uri, SAFE_LINK_URI);
}

#[test]
fn hostile_mode_rewrites_calendar_event_link_uri_to_file_scheme() {
    let server = MockServer::builder()
        .calendar_event_link_uses_file_uri()
        .start();
    let event_id = seed_calendar_event_with_link(&server);

    let uri = event_link_uri(&server, &event_id);

    assert_eq!(uri, HOSTILE_LINK_URI);
}
