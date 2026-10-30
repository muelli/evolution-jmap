// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify shared-account collection visibility and write restrictions.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarRights;
use jmap_proto::contacts::AddressBookRights;
use jmap_proto::mail::MailboxRights;
use serde_json::{Value, json};

struct ShareFixture {
    server: MockServer,
    shared_account_id: Id,
    shared_mailbox_id: Id,
    shared_book_id: Id,
    shared_calendar_id: Id,
}

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

fn get_json(server: &MockServer, authorization: Option<&str>, path: &str) -> Value {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");

    let mut request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {address}\r\n"
    );
    if let Some(authorization) = authorization {
        request.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");

    stream
        .write_all(request.as_bytes())
        .expect("write request bytes");
    stream.flush().expect("flush request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read response bytes");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("response has a header block");
    assert!(head.contains("200"), "{head}");
    serde_json::from_str(body).expect("session JSON body")
}

fn share_fixture() -> ShareFixture {
    let owner_principal = Id::from("P-owner");
    let grantee_principal = Id::from("P-grantee");
    let shared_account_id = Id::from("A2");

    let server = MockServer::builder()
        .bearer_token_as("grantee-token", grantee_principal.clone())
        .start();

    let (shared_mailbox_id, shared_book_id, shared_calendar_id) = {
        let state = server.state();
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        {
            let account = state
                .account_mut(&server.account_id())
                .expect("default account exists");
            account.current_user_principal_id = Some(grantee_principal.clone());
        }

        let shared = state.add_account(shared_account_id.clone(), "owner@example.test");
        shared.is_personal = false;
        shared.current_user_principal_id = Some(owner_principal);

        let shared_mailbox_id =
            shared.seed_mailbox("Shared Inbox", Some(jmap_proto::mail::role::INBOX));
        let _private_mailbox_id = shared.seed_mailbox("Private", None);
        let shared_book_id = shared.seed_address_book("Shared", false);
        let _private_book_id = shared.seed_address_book("Private", false);
        let shared_calendar_id = shared.seed_calendar("Shared", false);
        let _private_calendar_id = shared.seed_calendar("Private", false);

        let mailbox = shared
            .mailboxes
            .get_mut(&shared_mailbox_id)
            .expect("shared mailbox exists");
        mailbox.share_with = Some(BTreeMap::from([(
            grantee_principal.clone(),
            MailboxRights::read_only(),
        )]));

        let address_book = shared
            .address_books
            .get_mut(&shared_book_id)
            .expect("shared address book exists");
        address_book.share_with = Some(BTreeMap::from([(
            grantee_principal.clone(),
            AddressBookRights::read_only(),
        )]));

        let calendar = shared
            .calendars
            .get_mut(&shared_calendar_id)
            .expect("shared calendar exists");
        calendar.share_with = Some(BTreeMap::from([(
            grantee_principal,
            CalendarRights::read_only(),
        )]));

        (shared_mailbox_id, shared_book_id, shared_calendar_id)
    };

    ShareFixture {
        server,
        shared_account_id,
        shared_mailbox_id,
        shared_book_id,
        shared_calendar_id,
    }
}

#[test]
fn grantee_sees_only_granted_collections_with_my_rights() {
    let fixture = share_fixture();
    let account_id = fixture.shared_account_id.to_string();

    let session = get_json(
        &fixture.server,
        Some("Bearer grantee-token"),
        "/.well-known/jmap",
    );
    assert_eq!(session["accounts"]["A2"]["isPersonal"], json!(false));
    assert_eq!(
        session["accounts"]["A2"]["name"],
        json!("owner@example.test")
    );

    let response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:mail",
                "urn:ietf:params:jmap:contacts",
                "urn:ietf:params:jmap:calendars"
            ],
            "methodCalls": [
                ["Mailbox/get", {"accountId": account_id, "properties": ["id", "name", "myRights"]}, "m1"],
                ["AddressBook/get", {"accountId": "A2", "properties": ["id", "name", "myRights"]}, "ab1"],
                ["Calendar/get", {"accountId": "A2", "properties": ["id", "name", "myRights"]}, "c1"]
            ]
        }),
    );

    assert_eq!(response["methodResponses"][0][0], "Mailbox/get");
    assert_eq!(response["methodResponses"][1][0], "AddressBook/get");
    assert_eq!(response["methodResponses"][2][0], "Calendar/get");

    let mailbox_list = response["methodResponses"][0][1]["list"]
        .as_array()
        .expect("mailbox list");
    assert_eq!(mailbox_list.len(), 1);
    assert_eq!(
        mailbox_list[0]["id"],
        json!(fixture.shared_mailbox_id.as_str())
    );
    assert_eq!(mailbox_list[0]["myRights"]["mayReadItems"], json!(true));

    let book_list = response["methodResponses"][1][1]["list"]
        .as_array()
        .expect("address book list");
    assert_eq!(book_list.len(), 1);
    assert_eq!(book_list[0]["id"], json!(fixture.shared_book_id.as_str()));
    assert_eq!(book_list[0]["myRights"]["mayRead"], json!(true));

    let calendar_list = response["methodResponses"][2][1]["list"]
        .as_array()
        .expect("calendar list");
    assert_eq!(calendar_list.len(), 1);
    assert_eq!(
        calendar_list[0]["id"],
        json!(fixture.shared_calendar_id.as_str())
    );
    assert_eq!(calendar_list[0]["myRights"]["mayReadItems"], json!(true));
}

#[test]
fn grantee_writes_past_collection_rights_are_forbidden() {
    let fixture = share_fixture();
    let mailbox_id = fixture.shared_mailbox_id.to_string();
    let book_id = fixture.shared_book_id.to_string();
    let calendar_id = fixture.shared_calendar_id.to_string();

    let response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:mail",
                "urn:ietf:params:jmap:contacts",
                "urn:ietf:params:jmap:calendars"
            ],
            "methodCalls": [
                ["Mailbox/set", {"accountId": "A2", "update": {mailbox_id.clone(): {"name": "Renamed"}}}, "m1"],
                ["AddressBook/set", {"accountId": "A2", "update": {book_id.clone(): {"name": "Renamed"}}}, "ab1"],
                ["Calendar/set", {"accountId": "A2", "update": {calendar_id.clone(): {"name": "Renamed"}}}, "c1"]
            ]
        }),
    );

    assert_eq!(response["methodResponses"][0][0], "Mailbox/set");
    assert_eq!(response["methodResponses"][1][0], "AddressBook/set");
    assert_eq!(response["methodResponses"][2][0], "Calendar/set");

    assert_eq!(
        response["methodResponses"][0][1]["notUpdated"][mailbox_id.as_str()]["type"],
        json!("forbidden")
    );
    assert_eq!(
        response["methodResponses"][1][1]["notUpdated"][book_id.as_str()]["type"],
        json!("forbidden")
    );
    assert_eq!(
        response["methodResponses"][2][1]["notUpdated"][calendar_id.as_str()]["type"],
        json!("forbidden")
    );
}
