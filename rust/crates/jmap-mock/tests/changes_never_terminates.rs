// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `/changes` paging on the in-process mock server.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
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

fn mailbox_changes(server: &MockServer, since_state: &str) -> Value {
    let response = api(
        server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Mailbox/changes",
                {"accountId": "A1", "sinceState": since_state},
                "c1"
            ]],
        }),
    );
    response["methodResponses"][0][1].clone()
}

fn seed_two_mailbox_changes(server: &MockServer) {
    server.with_account_mut(server.account_id(), |account| {
        account.create_mailbox("One", None, None);
        account.create_mailbox("Two", None, None);
    });
}

#[test]
fn without_the_flag_changes_paging_terminates() {
    let server = MockServer::builder().changes_page_size(1).start();
    seed_two_mailbox_changes(&server);

    let first = mailbox_changes(&server, "1");
    assert_eq!(first["hasMoreChanges"], json!(true));
    assert_eq!(first["newState"], json!("2"));

    let second = mailbox_changes(
        &server,
        first["newState"].as_str().expect("string state token"),
    );
    assert_eq!(second["hasMoreChanges"], json!(false));
    assert_eq!(second["newState"], json!("3"));
}

#[test]
fn hostile_mode_keeps_changes_paging_non_terminating() {
    let server = MockServer::builder()
        .changes_page_size(1)
        .changes_never_terminates()
        .start();
    seed_two_mailbox_changes(&server);

    let first = mailbox_changes(&server, "1");
    assert_eq!(first["hasMoreChanges"], json!(true));
    assert_eq!(first["newState"], json!("1"));

    let second = mailbox_changes(
        &server,
        first["newState"].as_str().expect("string state token"),
    );
    assert_eq!(second["hasMoreChanges"], json!(true));
    assert_eq!(second["newState"], json!("1"));
    assert_eq!(second["created"], first["created"]);
}
