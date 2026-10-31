// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify shared-mailbox rights for `Email/*` methods.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::MailboxRights;
use serde_json::{Value, json};

struct ShareMailFixture {
    server: MockServer,
    shared_account_id: Id,
    read_only_mailbox_id: Id,
    read_write_mailbox_id: Id,
    read_only_email_id: Id,
    read_write_email_id: Id,
    private_email_id: Id,
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

fn share_mail_fixture() -> ShareMailFixture {
    let owner_principal = Id::from("P-owner");
    let grantee_principal = Id::from("P-grantee");
    let shared_account_id = Id::from("A2");

    let server = MockServer::builder()
        .bearer_token_as("grantee-token", grantee_principal.clone())
        .bearer_token_as("owner-token", owner_principal.clone())
        .start();

    let (
        read_only_mailbox_id,
        read_write_mailbox_id,
        read_only_email_id,
        read_write_email_id,
        private_email_id,
    ) = {
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
        shared.current_user_principal_id = Some(owner_principal.clone());

        let read_only_mailbox_id =
            shared.seed_mailbox("Shared Read Only", Some(jmap_proto::mail::role::INBOX));
        let read_write_mailbox_id = shared.seed_mailbox("Shared Read Write", None);
        let private_mailbox_id = shared.seed_mailbox("Private", None);

        let read_write_rights = MailboxRights::read_only()
            .with_may_add_items(true)
            .with_may_remove_items(true)
            .with_may_set_seen(true)
            .with_may_set_keywords(true)
            .with_may_create_child(true)
            .with_may_rename(true)
            .with_may_delete(true);

        let read_only_mailbox = shared
            .mailboxes
            .get_mut(&read_only_mailbox_id)
            .expect("shared read-only mailbox exists");
        read_only_mailbox.share_with = Some(BTreeMap::from([(
            grantee_principal.clone(),
            MailboxRights::read_only(),
        )]));

        let read_write_mailbox = shared
            .mailboxes
            .get_mut(&read_write_mailbox_id)
            .expect("shared read-write mailbox exists");
        read_write_mailbox.share_with =
            Some(BTreeMap::from([(grantee_principal, read_write_rights)]));

        let read_only_email_id = shared.seed_email(EmailSeed::new(
            read_only_mailbox_id.clone(),
            ("Owner", "owner@example.test"),
            "Read-only mailbox message",
            "Shared read-only mailbox message",
            "2026-08-01T09:00:00Z",
        ));
        shared.seed_email(EmailSeed::new(
            read_only_mailbox_id.clone(),
            ("Owner", "owner@example.test"),
            "Second read-only mailbox message",
            "Second shared read-only mailbox message",
            "2026-08-02T09:00:00Z",
        ));

        let read_write_email_id = shared.seed_email(EmailSeed::new(
            read_write_mailbox_id.clone(),
            ("Owner", "owner@example.test"),
            "Read-write mailbox message",
            "Shared read-write mailbox message",
            "2026-08-03T09:00:00Z",
        ));
        shared.seed_email(
            EmailSeed::new(
                read_write_mailbox_id.clone(),
                ("Owner", "owner@example.test"),
                "Second read-write mailbox message",
                "Second shared read-write mailbox message",
                "2026-08-04T09:00:00Z",
            )
            .keyword(jmap_proto::mail::keyword::SEEN),
        );

        let private_email_id = shared.seed_email(EmailSeed::new(
            private_mailbox_id,
            ("Owner", "owner@example.test"),
            "Private mailbox message",
            "Private mailbox message body",
            "2026-08-05T09:00:00Z",
        ));

        (
            read_only_mailbox_id,
            read_write_mailbox_id,
            read_only_email_id,
            read_write_email_id,
            private_email_id,
        )
    };

    ShareMailFixture {
        server,
        shared_account_id,
        read_only_mailbox_id,
        read_write_mailbox_id,
        read_only_email_id,
        read_write_email_id,
        private_email_id,
    }
}

#[test]
fn grantee_mail_methods_follow_shared_mailbox_rights() {
    let fixture = share_mail_fixture();

    let session = get_json(
        &fixture.server,
        Some("Bearer grantee-token"),
        "/.well-known/jmap",
    );
    assert!(
        session["accounts"]["A2"]["accountCapabilities"]["urn:ietf:params:jmap:mail"].is_object()
    );

    let account_id = fixture.shared_account_id.to_string();
    let read_only_mailbox_id = fixture.read_only_mailbox_id.to_string();
    let read_write_mailbox_id = fixture.read_write_mailbox_id.to_string();
    let read_only_email_id = fixture.read_only_email_id.to_string();
    let read_write_email_id = fixture.read_write_email_id.to_string();
    let private_email_id = fixture.private_email_id.to_string();

    let response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                ["Mailbox/get", {"accountId": account_id, "properties": ["id", "myRights"]}, "m1"],
                ["Email/query", {"accountId": "A2"}, "q1"],
                ["Email/get", {"accountId": "A2", "ids": [read_only_email_id, read_write_email_id, private_email_id]}, "g1"]
            ]
        }),
    );

    let mailboxes = response["methodResponses"][0][1]["list"]
        .as_array()
        .expect("mailboxes list");
    assert_eq!(mailboxes.len(), 2, "{response}");

    let read_only = mailboxes
        .iter()
        .find(|mailbox| mailbox["id"] == json!(read_only_mailbox_id))
        .expect("read-only mailbox is visible");
    assert_eq!(read_only["myRights"]["mayReadItems"], json!(true));
    assert_eq!(read_only["myRights"]["mayAddItems"], Value::Null);

    let read_write = mailboxes
        .iter()
        .find(|mailbox| mailbox["id"] == json!(read_write_mailbox_id))
        .expect("read-write mailbox is visible");
    assert_eq!(read_write["myRights"]["mayReadItems"], json!(true));
    assert_eq!(read_write["myRights"]["mayAddItems"], json!(true));
    assert_eq!(read_write["myRights"]["mayRemoveItems"], json!(true));
    assert_eq!(read_write["myRights"]["maySetSeen"], json!(true));
    assert_eq!(read_write["myRights"]["maySetKeywords"], json!(true));
    assert_eq!(read_write["myRights"]["mayCreateChild"], json!(true));
    assert_eq!(read_write["myRights"]["mayRename"], json!(true));
    assert_eq!(read_write["myRights"]["mayDelete"], json!(true));

    let query_ids = response["methodResponses"][1][1]["ids"]
        .as_array()
        .expect("query ids");
    assert_eq!(query_ids.len(), 4, "{response}");
    assert!(
        !query_ids.contains(&json!(private_email_id)),
        "private email must not leak: {response}"
    );

    let get_list = response["methodResponses"][2][1]["list"]
        .as_array()
        .expect("email get list");
    assert_eq!(get_list.len(), 2, "{response}");
    assert_eq!(
        response["methodResponses"][2][1]["notFound"],
        json!([private_email_id])
    );

    let old_state = response["methodResponses"][1][1]["queryState"]
        .as_str()
        .expect("query state")
        .to_owned();

    let set_response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Email/set",
                {
                    "accountId": "A2",
                    "create": {
                        "ro": {
                            "mailboxIds": {read_only_mailbox_id.clone(): true}
                        },
                        "rw": {
                            "mailboxIds": {read_write_mailbox_id.clone(): true}
                        }
                    },
                    "update": {
                        read_only_email_id.clone(): {"keywords/$seen": true},
                        read_write_email_id.clone(): {"keywords/$seen": true}
                    }
                },
                "s1"
            ]]
        }),
    );

    assert_eq!(
        set_response["methodResponses"][0][1]["notCreated"]["ro"]["type"],
        json!("forbidden")
    );
    assert_eq!(
        set_response["methodResponses"][0][1]["notUpdated"][read_only_email_id.as_str()]["type"],
        json!("forbidden")
    );
    assert_eq!(
        set_response["methodResponses"][0][1]["updated"][read_write_email_id.as_str()],
        Value::Null
    );

    let created_read_write_email_id = set_response["methodResponses"][0][1]["created"]["rw"]["id"]
        .as_str()
        .expect("read-write create id")
        .to_owned();

    let changes_response = api(
        &fixture.server,
        Some("Bearer grantee-token"),
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Email/changes",
                {
                    "accountId": "A2",
                    "sinceState": old_state
                },
                "c1"
            ]]
        }),
    );

    let created = changes_response["methodResponses"][0][1]["created"]
        .as_array()
        .expect("created ids");
    let updated = changes_response["methodResponses"][0][1]["updated"]
        .as_array()
        .expect("updated ids");
    assert!(
        created.contains(&json!(created_read_write_email_id)),
        "{changes_response}"
    );
    assert!(
        updated.contains(&json!(read_write_email_id)),
        "{changes_response}"
    );
    assert!(
        !updated.contains(&json!(read_only_email_id)),
        "read-only email update must be forbidden: {changes_response}"
    );

    let owner_view = api(
        &fixture.server,
        Some("Bearer owner-token"),
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Email/get",
                {
                    "accountId": "A2",
                    "ids": [read_write_email_id],
                    "properties": ["id", "keywords"]
                },
                "owner"
            ]]
        }),
    );
    assert_eq!(
        owner_view["methodResponses"][0][1]["list"][0]["keywords"]["$seen"],
        json!(true)
    );
}
