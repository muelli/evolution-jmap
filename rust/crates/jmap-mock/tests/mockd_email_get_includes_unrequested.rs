// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `Email/get` object padding on standalone `jmap-mockd`.

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

fn email_get(mockd: &Mockd, ids: &[&str]) -> Value {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Email/get",
                {
                    "accountId": "A1",
                    "ids": ids,
                    "properties": ["id", "subject"]
                },
                "g1"
            ]],
        }),
    );
    response["methodResponses"][0][1].clone()
}

fn email_ids(method_response: &Value) -> Vec<String> {
    method_response["list"]
        .as_array()
        .expect("list array")
        .iter()
        .map(|value| {
            value["id"]
                .as_str()
                .expect("email id is a string")
                .to_owned()
        })
        .collect()
}

#[test]
fn the_flag_makes_email_get_include_unrequested_objects() {
    let mockd = Mockd::spawn(&["--email-get-includes-unrequested"]);

    let response = email_get(&mockd, &["E1"]);

    assert_eq!(email_ids(&response), vec!["E1", "E2"]);
}

#[test]
fn help_lists_the_email_get_includes_unrequested_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(
        stderr.contains("--email-get-includes-unrequested"),
        "{stderr}"
    );
}
