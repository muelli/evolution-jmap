// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify `ContactCard` group-card behavior in the in-process mock server.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use serde_json::{Value, json};

const ACCOUNT_ID: &str = "A1";

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

fn address_book_ids(book_id: &str) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(book_id.to_owned(), Value::Bool(true));
    Value::Object(map)
}

fn create_card(server: &MockServer, card: Value) -> String {
    let response = api(
        server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [["ContactCard/set", {"accountId": ACCOUNT_ID, "create": {"k1": card}}, "s1"]],
        }),
    );

    response["methodResponses"][0][1]["created"]["k1"]["id"]
        .as_str()
        .expect("created card id")
        .to_owned()
}

#[cfg(feature = "parse")]
fn upload_blob(server: &MockServer, content_type: &str, body: &[u8]) -> String {
    let path = format!("/upload/{ACCOUNT_ID}");
    let (status, _, body) = request(server, "POST", &path, Some(content_type), body);
    assert!(status.contains("201"), "{status}");
    let response: Value = serde_json::from_slice(&body).expect("upload response JSON");
    response["blobId"]
        .as_str()
        .expect("upload blob id")
        .to_owned()
}

#[test]
fn contact_card_set_rejects_group_members_that_are_not_known_card_uids() {
    let server = MockServer::builder().start();
    let book_id = ensure_address_book_id(&server);
    let response = api(
        &server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "k1": {
                            "addressBookIds": address_book_ids(&book_id),
                            "uid": "urn:uuid:group-1",
                            "kind": "group",
                            "members": {
                                "urn:uuid:missing": true
                            },
                            "name": {"full": "Team Missing"}
                        }
                    }
                },
                "c1"
            ]],
        }),
    );

    let not_created = &response["methodResponses"][0][1]["notCreated"]["k1"];
    assert_eq!(not_created["type"], "invalidProperties");
    assert!(
        not_created["description"]
            .as_str()
            .is_some_and(|description| description.contains("members")),
        "{response:#}"
    );
}

#[test]
fn group_cards_roundtrip_through_set_get_query_and_changes() {
    let server = MockServer::builder().start();
    let book_id = ensure_address_book_id(&server);
    let uid_one = "urn:uuid:11111111-1111-1111-1111-111111111111";
    let uid_two = "urn:uuid:22222222-2222-2222-2222-222222222222";

    create_card(
        &server,
        json!({
            "addressBookIds": address_book_ids(&book_id),
            "uid": uid_one,
            "name": {"full": "Member One"}
        }),
    );
    create_card(
        &server,
        json!({
            "addressBookIds": address_book_ids(&book_id),
            "uid": uid_two,
            "name": {"full": "Member Two"}
        }),
    );

    let state_before = api(
        &server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [["ContactCard/query", {"accountId": ACCOUNT_ID}, "q0"]],
        }),
    )["methodResponses"][0][1]["queryState"]
        .as_str()
        .expect("query state before group create")
        .to_owned();

    let group_id = create_card(
        &server,
        json!({
            "addressBookIds": address_book_ids(&book_id),
            "uid": "urn:uuid:group-2",
            "kind": "group",
            "members": {
                uid_one: true,
                uid_two: true
            },
            "name": {"full": "Project Team"}
        }),
    );

    let get_response = api(
        &server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/get",
                {
                    "accountId": ACCOUNT_ID,
                    "ids": [group_id],
                    "properties": ["id", "kind", "members"]
                },
                "g1"
            ]],
        }),
    );

    let group = &get_response["methodResponses"][0][1]["list"][0];
    assert_eq!(group["kind"], "group");
    assert_eq!(group["members"][uid_one], Value::Bool(true));
    assert_eq!(group["members"][uid_two], Value::Bool(true));

    let query_response = api(
        &server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/query",
                {
                    "accountId": ACCOUNT_ID,
                    "filter": {"kind": "group"}
                },
                "q1"
            ]],
        }),
    );
    let ids = query_response["methodResponses"][0][1]["ids"]
        .as_array()
        .expect("group query ids");
    assert_eq!(ids, &vec![Value::String(group_id.clone())]);

    let changes_response = api(
        &server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/changes",
                {
                    "accountId": ACCOUNT_ID,
                    "sinceState": state_before
                },
                "h1"
            ]],
        }),
    );
    let created = changes_response["methodResponses"][0][1]["created"]
        .as_array()
        .expect("changes created list");
    assert!(
        created
            .iter()
            .any(|id| id == &Value::String(group_id.clone())),
        "{changes_response:#}"
    );

    let mut update = serde_json::Map::new();
    update.insert(
        group_id.clone(),
        json!({"members": {"urn:uuid:not-present": true}}),
    );
    let bad_update = api(
        &server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/set",
                {
                    "accountId": ACCOUNT_ID,
                    "update": update
                },
                "s2"
            ]],
        }),
    );
    let not_updated = &bad_update["methodResponses"][0][1]["notUpdated"][group_id];
    assert_eq!(not_updated["type"], "invalidProperties");
}

#[cfg(feature = "parse")]
#[test]
fn contact_card_parse_projects_group_kind_and_members() {
    let server = MockServer::builder().start();
    let blob_id = upload_blob(
        &server,
        "text/vcard",
        concat!(
            "BEGIN:VCARD\r\n",
            "VERSION:3.0\r\n",
            "UID:urn:uuid:group-from-parse\r\n",
            "FN:Group From Parse\r\n",
            "KIND:group\r\n",
            "MEMBER:urn:uuid:33333333-3333-3333-3333-333333333333\r\n",
            "MEMBER:mailto:alice@example.com\r\n",
            "X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:44444444-4444-4444-4444-444444444444\r\n",
            "END:VCARD\r\n"
        )
        .as_bytes(),
    );

    let response = api(
        &server,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [[
                "ContactCard/parse",
                {
                    "accountId": ACCOUNT_ID,
                    "blobIds": [blob_id]
                },
                "p1"
            ]],
        }),
    );

    let parsed = response["methodResponses"][0][1]["parsed"]
        .as_object()
        .and_then(|map| map.get(&blob_id))
        .expect("parsed blob entry");
    assert_eq!(parsed["kind"], "group");

    let members = parsed["members"].as_object().expect("members map");
    assert_eq!(
        members.get("urn:uuid:33333333-3333-3333-3333-333333333333"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        members.get("urn:uuid:44444444-4444-4444-4444-444444444444"),
        Some(&Value::Bool(true))
    );
    assert!(!members.contains_key("mailto:alice@example.com"));
}
