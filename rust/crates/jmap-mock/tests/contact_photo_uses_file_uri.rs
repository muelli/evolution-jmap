// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `ContactCard/get` photo URIs on the in-process mock server.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use jmap_proto::contacts::{ContactCard, Media};
use serde_json::{Value, json};

const SAFE_PHOTO_URI: &str = "data:image/png;base64,Zm9v";
const HOSTILE_PHOTO_URI: &str = "file:///home/victim/.ssh/id_ed25519";

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

fn seed_contact_card_with_photo(server: &MockServer) -> String {
    let state = server.state();
    let mut state = state.lock().expect("state lock");
    let account = state
        .account_mut(&server.account_id())
        .expect("default account exists");

    let book_id = account.seed_address_book("Personal", true);
    let card_id = account.contact_cards.alloc_id();

    let mut media = BTreeMap::new();
    media.insert(
        "m1".to_owned(),
        Media::new(SAFE_PHOTO_URI).with_kind("photo"),
    );

    let card = ContactCard {
        id: Some(card_id.clone()),
        address_book_ids: Some(BTreeMap::from([(book_id, true)])),
        media: Some(media),
        ..ContactCard::default()
    };
    account.contact_cards.seed_with_id(card_id.clone(), card);

    card_id.to_string()
}

fn contact_photo_uri(server: &MockServer, card_id: &str) -> String {
    let response = api(
        server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/get",
                {
                    "accountId": "A1",
                    "ids": [card_id],
                    "properties": ["id", "media"]
                },
                "c1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["media"]["m1"]["uri"]
        .as_str()
        .expect("photo URI as string")
        .to_owned()
}

#[test]
fn without_the_flag_contact_photo_uri_stays_unchanged() {
    let server = MockServer::builder().start();
    let card_id = seed_contact_card_with_photo(&server);

    let uri = contact_photo_uri(&server, &card_id);

    assert_eq!(uri, SAFE_PHOTO_URI);
}

#[test]
fn hostile_mode_rewrites_contact_photo_uri_to_file_scheme() {
    let server = MockServer::builder().contact_photo_uses_file_uri().start();
    let card_id = seed_contact_card_with_photo(&server);

    let uri = contact_photo_uri(&server, &card_id);

    assert_eq!(uri, HOSTILE_PHOTO_URI);
}
