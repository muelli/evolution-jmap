// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify `jmap-mockd` task-mode flag behavior.

use serde_json::{Value, json};

mod common;

use common::Mockd;

const ACCOUNT_ID: &str = "A1";
const CAP_CORE: &str = "urn:ietf:params:jmap:core";
const CAP_TASKS: &str = "urn:ietf:params:jmap:tasks";

fn api(mockd: &Mockd, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a response JSON body")
}

fn session(mockd: &Mockd) -> Value {
    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a session JSON body")
}

#[test]
fn tasks_capability_is_hidden_without_flag() {
    let mockd = Mockd::spawn(&[]);
    let session = session(&mockd);

    assert!(session["capabilities"].get(CAP_TASKS).is_none());
    assert!(
        session["accounts"][ACCOUNT_ID]["accountCapabilities"]
            .get(CAP_TASKS)
            .is_none()
    );
}

#[test]
fn tasks_flag_enables_capability_and_task_list_set() {
    let mockd = Mockd::spawn(&["--tasks"]);
    let session = session(&mockd);

    assert!(session["capabilities"].get(CAP_TASKS).is_some());
    assert!(
        session["accounts"][ACCOUNT_ID]["accountCapabilities"]
            .get(CAP_TASKS)
            .is_some()
    );

    let set = api(
        &mockd,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "TaskList/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "l1": {"name": "From mockd"}
                    }
                },
                "l1"
            ]],
        }),
    );
    assert!(set["methodResponses"][0][1]["created"]["l1"]["id"].is_string());
}
