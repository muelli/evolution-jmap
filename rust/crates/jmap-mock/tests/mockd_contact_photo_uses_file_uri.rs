// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile contact-photo URI flags on standalone `jmap-mockd`.

use std::process::Command;

use serde_json::{Value, json};

mod common;

use common::Mockd;

const SAFE_PHOTO_URI: &str = "data:image/png;base64,Zm9v";
const HOSTILE_PHOTO_URI: &str = "file:///home/victim/.ssh/id_ed25519";

fn api(mockd: &Mockd, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a response JSON body")
}

fn create_card_with_photo(mockd: &Mockd) -> String {
    let book_id = default_address_book_id(mockd);
    let mut address_book_ids = serde_json::Map::new();
    address_book_ids.insert(book_id, Value::Bool(true));

    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/set",
                {
                    "accountId": "A1",
                    "create": {
                        "k1": {
                            "addressBookIds": address_book_ids,
                            "media": {"m1": {"kind": "photo", "uri": SAFE_PHOTO_URI}}
                        }
                    }
                },
                "s1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["created"]["k1"]["id"]
        .as_str()
        .expect("created card id")
        .to_owned()
}

fn default_address_book_id(mockd: &Mockd) -> String {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "AddressBook/get",
                {"accountId": "A1", "properties": ["id"]},
                "ab1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["id"]
        .as_str()
        .expect("default address book id")
        .to_owned()
}

fn contact_photo_uri(mockd: &Mockd, card_id: &str) -> String {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/get",
                {
                    "accountId": "A1",
                    "ids": [card_id],
                    "properties": ["id", "media"]
                },
                "g1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["media"]["m1"]["uri"]
        .as_str()
        .expect("photo URI as string")
        .to_owned()
}

#[test]
fn the_flag_rewrites_contact_photo_uri_to_file_scheme() {
    let mockd = Mockd::spawn(&["--contact-photo-uses-file-uri"]);
    let card_id = create_card_with_photo(&mockd);

    let uri = contact_photo_uri(&mockd, &card_id);

    assert_eq!(uri, HOSTILE_PHOTO_URI);
}

#[test]
fn help_lists_the_contact_photo_uses_file_uri_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--contact-photo-uses-file-uri"), "{stderr}");
}
