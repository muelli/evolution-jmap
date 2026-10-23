// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile `Email/query` flags on standalone `jmap-mockd`.

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

fn email_query(mockd: &Mockd, position: u64) -> Value {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[
                "Email/query",
                {"accountId": "A1", "position": position, "limit": 4},
                "q1"
            ]],
        }),
    );
    response["methodResponses"][0][1].clone()
}

#[test]
fn the_flag_keeps_query_paging_non_terminating() {
    let mockd = Mockd::spawn(&["--query-page-size", "1", "--query-never-terminates"]);

    let first = email_query(&mockd, 0);
    assert_eq!(first["limit"], json!(1));
    assert_eq!(first["ids"].as_array().map(Vec::len), Some(1));

    let second = email_query(&mockd, 1);
    assert_eq!(second["limit"], json!(1));
    assert_eq!(second["ids"].as_array().map(Vec::len), Some(1));
    assert_eq!(second["ids"], first["ids"]);
}

#[test]
fn help_lists_the_query_never_terminates_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--query-page-size"), "{stderr}");
    assert!(stderr.contains("--query-never-terminates"), "{stderr}");
}
