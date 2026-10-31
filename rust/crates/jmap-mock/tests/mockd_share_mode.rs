// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify `jmap-mockd --share` seeds a shared-account scenario.

use std::process::Command;

use serde_json::{Value, json};

mod common;

use common::Mockd;

fn api(mockd: &Mockd, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a response JSON body")
}

#[test]
fn share_mode_seeds_a_non_personal_shared_account_with_collection_grants() {
    let mockd = Mockd::spawn(&["--share"]);

    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");
    assert!(status.contains("200"), "{status}");

    let session: Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["accounts"]["A2"]["isPersonal"], json!(false));

    let response = api(
        &mockd,
        json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:mail",
                "urn:ietf:params:jmap:contacts",
                "urn:ietf:params:jmap:calendars"
            ],
            "methodCalls": [
                ["Mailbox/get", {"accountId": "A2", "properties": ["id", "shareWith"]}, "m1"],
                ["AddressBook/get", {"accountId": "A2", "properties": ["id", "shareWith"]}, "ab1"],
                ["Calendar/get", {"accountId": "A2", "properties": ["id", "shareWith"]}, "c1"]
            ]
        }),
    );

    let mailbox_has_grant = response["methodResponses"][0][1]["list"]
        .as_array()
        .expect("mailboxes list")
        .iter()
        .any(|item| item["shareWith"].is_object());
    let address_book_has_grant = response["methodResponses"][1][1]["list"]
        .as_array()
        .expect("address books list")
        .iter()
        .any(|item| item["shareWith"].is_object());
    let calendar_has_grant = response["methodResponses"][2][1]["list"]
        .as_array()
        .expect("calendars list")
        .iter()
        .any(|item| item["shareWith"].is_object());

    assert!(mailbox_has_grant, "{response}");
    assert!(address_book_has_grant, "{response}");
    assert!(calendar_has_grant, "{response}");
}

#[test]
fn share_mode_seeds_read_only_and_read_write_mailboxes_with_messages() {
    let mockd = Mockd::spawn(&["--share"]);

    let response = api(
        &mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                ["Mailbox/get", {"accountId": "A2", "properties": ["id", "name", "shareWith"]}, "m1"],
                ["Email/query", {"accountId": "A2"}, "q1"]
            ]
        }),
    );

    let mailbox_list = response["methodResponses"][0][1]["list"]
        .as_array()
        .expect("mailboxes list");
    let shared_mailboxes: Vec<&Value> = mailbox_list
        .iter()
        .filter(|mailbox| mailbox["shareWith"].is_object())
        .collect();
    assert_eq!(shared_mailboxes.len(), 2, "{response}");

    let read_only = shared_mailboxes
        .iter()
        .find(|mailbox| mailbox["name"] == json!("Shared Read Only"))
        .expect("read-only shared mailbox");
    assert_eq!(
        read_only["shareWith"]["P-grantee"]["mayReadItems"],
        json!(true)
    );
    assert_eq!(
        read_only["shareWith"]["P-grantee"]["mayAddItems"],
        Value::Null
    );

    let read_write = shared_mailboxes
        .iter()
        .find(|mailbox| mailbox["name"] == json!("Shared Read Write"))
        .expect("read-write shared mailbox");
    assert_eq!(
        read_write["shareWith"]["P-grantee"]["mayReadItems"],
        json!(true)
    );
    assert_eq!(
        read_write["shareWith"]["P-grantee"]["mayAddItems"],
        json!(true)
    );
    assert_eq!(
        read_write["shareWith"]["P-grantee"]["mayRemoveItems"],
        json!(true)
    );
    assert_eq!(
        read_write["shareWith"]["P-grantee"]["maySetSeen"],
        json!(true)
    );
    assert_eq!(
        read_write["shareWith"]["P-grantee"]["maySetKeywords"],
        json!(true)
    );

    let query_ids = response["methodResponses"][1][1]["ids"]
        .as_array()
        .expect("email query ids");
    assert_eq!(query_ids.len(), 4, "{response}");
}

#[test]
fn help_lists_the_share_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--share"), "{stderr}");
}
