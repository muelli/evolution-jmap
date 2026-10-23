// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `Email/get` object padding on the in-process mock server.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::{EmailSeed, MockServer};
use serde_json::{Value, json};

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

fn email_get(server: &MockServer, ids: &[&str]) -> Value {
    let response = api(
        server,
        json!({
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
        }),
    );
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

fn seed_two_emails(server: &MockServer) {
    server.with_account_mut(server.account_id(), |account| {
        let inbox = account.seed_mailbox("Inbox", Some(jmap_proto::mail::role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "First",
            "First body",
            "2026-08-01T10:00:00Z",
        ));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Carol", "carol@example.com"),
            "Second",
            "Second body",
            "2026-08-02T10:00:00Z",
        ));
    });
}

#[test]
fn without_the_flag_email_get_returns_only_requested_objects() {
    let server = MockServer::builder().start();
    seed_two_emails(&server);

    let response = email_get(&server, &["E1"]);

    assert_eq!(email_ids(&response), vec!["E1"]);
}

#[test]
fn hostile_mode_email_get_includes_unrequested_objects() {
    let server = MockServer::builder()
        .email_get_includes_unrequested()
        .start();
    seed_two_emails(&server);

    let response = email_get(&server, &["E1"]);

    assert_eq!(email_ids(&response), vec!["E1", "E2"]);
}
