// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify `AddressBook/get` treats an owner principal as the owner.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use jmap_proto::Id;
use serde_json::{Value, json};

fn api(server: &MockServer, authorization: Option<&str>, body: Value) -> Value {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");

    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let mut headers = format!(
        "POST /jmap HTTP/1.1\r\n\
         Host: {address}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n",
        payload.len()
    );
    if let Some(authorization) = authorization {
        headers.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    headers.push_str("Connection: close\r\n\r\n");

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

#[test]
fn address_book_get_accepts_a_token_identity_that_matches_the_owner_principal() {
    let owner_principal = Id::from("P-owner");
    let server = MockServer::builder()
        .bearer_token_as("owner-token", owner_principal.clone())
        .start();
    let book_id = server.with_account_mut(server.account_id(), |account| {
        account.current_user_principal_id = Some(owner_principal);
        account.seed_address_book("Personal", true).to_string()
    });

    let response = api(
        &server,
        Some("Bearer owner-token"),
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "AddressBook/get",
                {
                    "accountId": "A1",
                    "ids": [book_id],
                    "properties": ["id", "name", "myRights"]
                },
                "c1"
            ]]
        }),
    );

    assert_eq!(response["methodResponses"][0][0], "AddressBook/get");
    assert_eq!(
        response["methodResponses"][0][1]["list"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        response["methodResponses"][0][1]["notFound"].as_array(),
        Some(&Vec::new())
    );
}
