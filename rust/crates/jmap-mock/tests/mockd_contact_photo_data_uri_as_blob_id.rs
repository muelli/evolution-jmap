// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify contact-photo `data:` to `blobId` mode on standalone `jmap-mockd`.

use std::process::Command;

use serde_json::{Value, json};

mod common;

use common::Mockd;

const PHOTO_DATA_URI: &str = "data:image/png;base64,Zm9v";

fn api(mockd: &Mockd, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a response JSON body")
}

fn default_address_book_id(mockd: &Mockd) -> String {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [["AddressBook/get", {"accountId": "A1", "properties": ["id"]}, "ab1"]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["id"]
        .as_str()
        .expect("default address book id")
        .to_owned()
}

fn create_card_with_data_photo(mockd: &Mockd) -> String {
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
                            "media": {"m1": {"kind": "photo", "uri": PHOTO_DATA_URI}}
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

fn contact_photo_entry(mockd: &Mockd, card_id: &str) -> Value {
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

    response["methodResponses"][0][1]["list"][0]["media"]["m1"].clone()
}

#[test]
fn the_flag_serves_data_photo_as_blob_id_and_downloads_bytes() {
    let mockd = Mockd::spawn(&["--contact-photo-data-uri-as-blob-id"]);
    let card_id = create_card_with_data_photo(&mockd);

    let photo = contact_photo_entry(&mockd, &card_id);
    let blob_id = photo["blobId"].as_str().expect("photo blob id").to_owned();
    assert!(photo.get("uri").is_none());
    assert_eq!(photo["mediaType"], "image/png");

    let path = format!("/download/A1/{blob_id}/photo.bin");
    let (status, headers, body) = mockd.request("GET", &path, b"");
    assert!(status.contains("200"), "{status}");
    assert!(
        headers
            .iter()
            .any(|header| header.eq_ignore_ascii_case("content-type: image/png")),
        "download content type: {headers:?}"
    );
    assert_eq!(body, "foo");
}

#[test]
fn help_lists_the_contact_photo_data_uri_as_blob_id_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(
        stderr.contains("--contact-photo-data-uri-as-blob-id"),
        "{stderr}"
    );
}
