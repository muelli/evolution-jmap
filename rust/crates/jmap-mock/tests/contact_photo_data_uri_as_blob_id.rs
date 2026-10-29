// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify the `data:` photo to `blobId` mode on the in-process mock server.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use serde_json::{Value, json};

const ACCOUNT_ID: &str = "A1";
const PHOTO_DATA_URI: &str = "data:image/png;base64,Zm9v";

fn request(
    server: &MockServer,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> (String, Vec<String>, Vec<u8>) {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");

    let mut head = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    if let Some(content_type) = content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");

    stream.write_all(head.as_bytes()).expect("write headers");
    if !body.is_empty() {
        stream.write_all(body).expect("write body");
    }
    stream.flush().expect("flush request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read the response");

    let header_end = response
        .windows(4)
        .position(|chunk| chunk == b"\r\n\r\n")
        .expect("a response with a header block");
    let (head, body) = response.split_at(header_end + 4);
    let head = String::from_utf8(head[..head.len() - 4].to_vec()).expect("utf8 HTTP headers");
    let mut lines = head.lines();
    let status = lines.next().expect("a status line").to_owned();
    let headers = lines.map(str::to_owned).collect();

    (status, headers, body.to_vec())
}

fn api(server: &MockServer, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = request(server, "POST", "/jmap", Some("application/json"), &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_slice(&body).expect("a response JSON body")
}

fn ensure_address_book_id(server: &MockServer) -> String {
    server.with_account_mut(server.account_id(), |account| {
        if let Some((id, _)) = account.address_books.iter().next() {
            id.to_string()
        } else {
            account.seed_address_book("Personal", true).to_string()
        }
    })
}

fn create_card_with_media(server: &MockServer, media_entry: Value) -> String {
    let book_id = ensure_address_book_id(server);
    let mut address_book_ids = serde_json::Map::new();
    address_book_ids.insert(book_id, Value::Bool(true));

    let response = api(
        server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "k1": {
                            "addressBookIds": address_book_ids,
                            "media": {"m1": media_entry}
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

fn contact_photo_entry(server: &MockServer, card_id: &str) -> Value {
    let response = api(
        server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/get",
                {
                    "accountId": ACCOUNT_ID,
                    "ids": [card_id],
                    "properties": ["id", "media"]
                },
                "g1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["media"]["m1"].clone()
}

fn upload_blob(server: &MockServer, content_type: &str, body: &[u8]) -> String {
    let path = format!("/upload/{ACCOUNT_ID}");
    let (status, _, body) = request(server, "POST", &path, Some(content_type), body);
    assert!(status.contains("201"), "{status}");
    let response: Value = serde_json::from_slice(&body).expect("upload JSON response");
    response["blobId"]
        .as_str()
        .expect("upload blob id")
        .to_owned()
}

fn download_blob(server: &MockServer, blob_id: &str) -> (String, Vec<String>, Vec<u8>) {
    let path = format!("/download/{ACCOUNT_ID}/{blob_id}/photo.bin");
    request(server, "GET", &path, None, b"")
}

#[test]
fn without_the_flag_contact_photo_data_uri_stays_a_uri() {
    let server = MockServer::builder().start();
    let card_id = create_card_with_media(&server, json!({"kind": "photo", "uri": PHOTO_DATA_URI}));

    let entry = contact_photo_entry(&server, &card_id);

    assert_eq!(entry["uri"], PHOTO_DATA_URI);
    assert!(entry.get("blobId").is_none());
}

#[test]
fn the_flag_serves_a_data_photo_as_blob_id_with_downloadable_bytes() {
    let server = MockServer::builder()
        .contact_photo_data_uri_as_blob_id()
        .start();
    let card_id = create_card_with_media(&server, json!({"kind": "photo", "uri": PHOTO_DATA_URI}));

    let entry = contact_photo_entry(&server, &card_id);
    let blob_id = entry["blobId"].as_str().expect("photo blob id").to_owned();
    assert_eq!(entry["mediaType"], "image/png");
    assert!(entry.get("uri").is_none());

    let (status, headers, body) = download_blob(&server, &blob_id);
    assert!(status.contains("200"), "{status}");
    assert!(
        headers
            .iter()
            .any(|header| header.eq_ignore_ascii_case("content-type: image/png")),
        "download content type: {headers:?}"
    );
    assert_eq!(body, b"foo");
}

#[test]
fn the_flag_keeps_uploaded_photo_blob_ids_unchanged() {
    let server = MockServer::builder()
        .contact_photo_data_uri_as_blob_id()
        .start();
    let blob_id = upload_blob(&server, "image/png", b"raw-photo");
    let card_id = create_card_with_media(
        &server,
        json!({"kind": "photo", "blobId": blob_id, "mediaType": "image/png"}),
    );

    let entry = contact_photo_entry(&server, &card_id);
    let returned_blob_id = entry["blobId"].as_str().expect("stored photo blob id");
    assert_eq!(returned_blob_id, blob_id);
    assert_eq!(entry["mediaType"], "image/png");
    assert!(entry.get("uri").is_none());

    let (status, _, body) = download_blob(&server, returned_blob_id);
    assert!(status.contains("200"), "{status}");
    assert_eq!(body, b"raw-photo");
}
