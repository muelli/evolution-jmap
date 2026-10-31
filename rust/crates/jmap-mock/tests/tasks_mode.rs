// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify opt-in Task and TaskList support in the in-process mock server.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use serde_json::{Value, json};

const ACCOUNT_ID: &str = "A1";
const CAP_CORE: &str = "urn:ietf:params:jmap:core";
const CAP_TASKS: &str = "urn:ietf:params:jmap:tasks";

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

fn session(server: &MockServer) -> Value {
    let (status, _, body) = request(server, "GET", "/.well-known/jmap", None, b"");
    assert!(status.contains("200"), "{status}");
    serde_json::from_slice(&body).expect("a session JSON body")
}

#[test]
fn tasks_are_off_by_default() {
    let server = MockServer::builder().start();
    let session = session(&server);

    assert!(session["capabilities"].get(CAP_TASKS).is_none());
    assert!(
        session["accounts"][ACCOUNT_ID]["accountCapabilities"]
            .get(CAP_TASKS)
            .is_none()
    );

    let response = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [["TaskList/get", {"accountId": ACCOUNT_ID}, "t1"]],
        }),
    );
    assert_eq!(response["methodResponses"][0][0], "error");
    assert_eq!(response["methodResponses"][0][1]["type"], "unknownMethod");
}

#[test]
fn tasks_mode_supports_task_list_and_task_lifecycle() {
    let server = MockServer::builder().tasks().start();
    let session = session(&server);

    assert!(session["capabilities"].get(CAP_TASKS).is_some());
    assert!(
        session["accounts"][ACCOUNT_ID]["accountCapabilities"]
            .get(CAP_TASKS)
            .is_some()
    );

    let query_before = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [["Task/query", {"accountId": ACCOUNT_ID}, "q0"]],
        }),
    );
    let since_state = query_before["methodResponses"][0][1]["queryState"]
        .as_str()
        .expect("initial task query state")
        .to_owned();

    let list_set = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "TaskList/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "l1": {"name": "Backlog"}
                    }
                },
                "l1"
            ]],
        }),
    );
    let list_id = list_set["methodResponses"][0][1]["created"]["l1"]["id"]
        .as_str()
        .expect("created task list id")
        .to_owned();

    let task_set = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "Task/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "t1": {
                            "taskListId": list_id,
                            "uid": "urn:uuid:task-1",
                            "title": "Write tests",
                            "status": "needs-action",
                            "due": "2026-11-01T10:00:00Z"
                        }
                    }
                },
                "t1"
            ]],
        }),
    );
    let task_id = task_set["methodResponses"][0][1]["created"]["t1"]["id"]
        .as_str()
        .expect("created task id")
        .to_owned();

    let get = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "Task/get",
                {
                    "accountId": ACCOUNT_ID,
                    "ids": [task_id],
                    "properties": ["id", "taskListId", "title", "status"]
                },
                "g1"
            ]],
        }),
    );
    assert_eq!(
        get["methodResponses"][0][1]["list"][0]["title"],
        "Write tests"
    );

    let query = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "Task/query",
                {
                    "accountId": ACCOUNT_ID,
                    "filter": {
                        "taskListId": list_id,
                        "status": "needs-action",
                        "text": "write"
                    }
                },
                "q1"
            ]],
        }),
    );
    let ids = query["methodResponses"][0][1]["ids"]
        .as_array()
        .expect("task query ids");
    assert_eq!(ids, &vec![Value::String(task_id.clone())]);

    let changes = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "Task/changes",
                {
                    "accountId": ACCOUNT_ID,
                    "sinceState": since_state
                },
                "c1"
            ]],
        }),
    );
    let created = changes["methodResponses"][0][1]["created"]
        .as_array()
        .expect("task changes created list");
    assert!(
        created
            .iter()
            .any(|value| value == &Value::String(task_id.clone())),
        "{changes:#}"
    );
}

#[test]
fn task_set_rejects_unknown_task_list_id() {
    let server = MockServer::builder().tasks().start();

    let response = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "Task/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "t1": {
                            "taskListId": "TL404",
                            "uid": "urn:uuid:task-missing",
                            "title": "Orphan"
                        }
                    }
                },
                "s1"
            ]],
        }),
    );

    assert_eq!(
        response["methodResponses"][0][1]["notCreated"]["t1"]["type"],
        "taskListNotFound"
    );
}

#[test]
fn task_set_update_rejects_unknown_task_list_id() {
    let server = MockServer::builder().tasks().start();

    let list_set = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "TaskList/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "l1": {"name": "Backlog"}
                    }
                },
                "l1"
            ]],
        }),
    );
    let list_id = list_set["methodResponses"][0][1]["created"]["l1"]["id"]
        .as_str()
        .expect("created task list id")
        .to_owned();

    let created = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "Task/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "t1": {
                            "taskListId": list_id,
                            "uid": "urn:uuid:task-update",
                            "title": "Keep me"
                        }
                    }
                },
                "s1"
            ]],
        }),
    );
    let task_id = created["methodResponses"][0][1]["created"]["t1"]["id"]
        .as_str()
        .expect("created task id")
        .to_owned();

    let mut update = serde_json::Map::new();
    update.insert(task_id.clone(), json!({"taskListId": "TL404"}));

    let update_response = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_TASKS],
            "methodCalls": [[
                "Task/set",
                {
                    "accountId": ACCOUNT_ID,
                    "update": update
                },
                "s2"
            ]],
        }),
    );

    assert_eq!(
        update_response["methodResponses"][0][1]["notUpdated"][task_id.as_str()]["type"],
        "taskListNotFound"
    );
}
