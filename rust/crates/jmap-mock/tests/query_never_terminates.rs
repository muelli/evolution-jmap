// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `Email/query` paging on the in-process mock server.

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

fn email_query(server: &MockServer, position: u64) -> Value {
    let response = api(
        server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Email/query",
                {"accountId": "A1", "position": position, "limit": 4},
                "q1"
            ]],
        }),
    );
    response["methodResponses"][0][1].clone()
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
fn without_the_flag_query_paging_advances() {
    let server = MockServer::builder().query_page_size(1).start();
    seed_two_emails(&server);

    let first = email_query(&server, 0);
    assert_eq!(first["limit"], json!(1));
    assert_eq!(first["ids"].as_array().map(Vec::len), Some(1));

    let second = email_query(&server, 1);
    assert_eq!(second["limit"], json!(1));
    assert_eq!(second["ids"].as_array().map(Vec::len), Some(1));
    assert_ne!(second["ids"], first["ids"]);
}

#[test]
fn hostile_mode_keeps_query_paging_non_terminating() {
    let server = MockServer::builder()
        .query_page_size(1)
        .query_never_terminates()
        .start();
    seed_two_emails(&server);

    let first = email_query(&server, 0);
    assert_eq!(first["limit"], json!(1));
    assert_eq!(first["ids"].as_array().map(Vec::len), Some(1));

    let second = email_query(&server, 1);
    assert_eq!(second["limit"], json!(1));
    assert_eq!(second["ids"].as_array().map(Vec::len), Some(1));
    assert_eq!(second["ids"], first["ids"]);
}
