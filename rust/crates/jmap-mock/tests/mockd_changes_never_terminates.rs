// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `/changes` flags on standalone `jmap-mockd`.

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

fn mailbox_set(mockd: &Mockd, creation_id: &str, name: &str) {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Mailbox/set",
                {
                    "accountId": "A1",
                    "create": {
                        creation_id: {
                            "name": name,
                            "isSubscribed": true
                        }
                    }
                },
                creation_id
            ]],
        }),
    );

    let method_response = &response["methodResponses"][0];
    assert_eq!(method_response[0], json!("Mailbox/set"));
    assert_eq!(method_response[1]["notCreated"], Value::Null);
}

fn mailbox_changes(mockd: &Mockd, since_state: &str) -> Value {
    let response = api(
        mockd,
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

#[test]
fn the_flag_keeps_changes_paging_non_terminating() {
    let mockd = Mockd::spawn(&["--changes-page-size", "1", "--changes-never-terminates"]);

    mailbox_set(&mockd, "k1", "One");
    mailbox_set(&mockd, "k2", "Two");

    let first = mailbox_changes(&mockd, "1");
    assert_eq!(first["hasMoreChanges"], json!(true));
    assert_eq!(first["newState"], json!("1"));

    let second = mailbox_changes(
        &mockd,
        first["newState"].as_str().expect("string state token"),
    );
    assert_eq!(second["hasMoreChanges"], json!(true));
    assert_eq!(second["newState"], json!("1"));
    assert_eq!(second["created"], first["created"]);
}

#[test]
fn help_lists_the_changes_never_terminates_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--changes-never-terminates"), "{stderr}");
}
