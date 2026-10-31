// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify jmap-mockd CLI flags for principals and delegation mode.

mod common;

use std::process::Command;

use common::Mockd;
use serde_json::{Value, json};

#[test]
fn help_lists_the_disable_principal_query_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--disable-principal-query"), "{stderr}");
}

#[test]
fn disable_principal_query_flag_rejects_query() {
    let mockd = Mockd::spawn(&["--disable-principal-query"]);

    let payload = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:principals"
        ],
        "methodCalls": [
            ["Principal/query", {"accountId": "A1", "filter": null}, "p1"]
        ]
    });
    let body = serde_json::to_vec(&payload).expect("serialize payload");
    let (status, _headers, response) = mockd.request("POST", "/jmap", &body);
    assert!(status.contains("200"), "expected HTTP 200, got: {status}");

    let parsed: Value = serde_json::from_str(&response).expect("valid JSON response");
    let method_response = &parsed["methodResponses"][0];
    assert_eq!(method_response[0], "error");
    assert_eq!(method_response[1]["type"], "forbidden");
    assert_eq!(
        method_response[1]["description"],
        "The administrator has disabled directory queries."
    );
}

#[test]
fn share_mode_seeds_principals_and_calendar_events_for_availability() {
    let mockd = Mockd::spawn(&["--share"]);

    let payload = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
            "urn:ietf:params:jmap:principals"
        ],
        "methodCalls": [
            [
                "Principal/getAvailability",
                {
                    "accountId": "A1",
                    "id": "P-owner",
                    "utcStart": "2026-08-01T00:00:00Z",
                    "utcEnd": "2026-08-05T00:00:00Z",
                    "showDetails": true
                },
                "a1"
            ]
        ]
    });
    let body = serde_json::to_vec(&payload).expect("serialize payload");
    let (status, _headers, response) = mockd.request("POST", "/jmap", &body);
    assert!(status.contains("200"), "expected HTTP 200, got: {status}");

    let parsed: Value = serde_json::from_str(&response).expect("valid JSON response");
    let list = parsed["methodResponses"][0][1]["list"]
        .as_array()
        .expect("availability list");
    assert!(
        !list.is_empty(),
        "expected seeded availability busy periods"
    );
}
