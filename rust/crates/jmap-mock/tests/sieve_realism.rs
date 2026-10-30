// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validate Sieve capability and script-validation realism modes.

use std::io::{Read, Write};
use std::net::TcpStream;

use jmap_mock::MockServer;
use serde_json::{Value, json};

const ACCOUNT_ID: &str = "A1";
const CAP_CORE: &str = "urn:ietf:params:jmap:core";
const CAP_SIEVE: &str = "urn:ietf:params:jmap:sieve";
const CAP_VACATION: &str = "urn:ietf:params:jmap:vacationresponse";

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

fn upload_blob(server: &MockServer, content_type: &str, data: &[u8]) -> String {
    let path = format!("/upload/{ACCOUNT_ID}");
    let (status, _, body) = request(server, "POST", &path, Some(content_type), data);
    assert!(status.contains("201"), "{status}");
    let response: Value = serde_json::from_slice(&body).expect("upload JSON response");
    response["blobId"]
        .as_str()
        .expect("upload blob id")
        .to_owned()
}

fn set_create(server: &MockServer, name: &str, blob_id: &str) -> Value {
    api(
        server,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/set",
                {
                    "accountId": ACCOUNT_ID,
                    "create": {
                        "k1": {
                            "name": name,
                            "blobId": blob_id,
                        }
                    }
                },
                "s1"
            ]],
        }),
    )
}

fn set_error_type(value: &Value, section: &str, key: &str) -> String {
    value["methodResponses"][0][1][section][key]["type"]
        .as_str()
        .expect("set error type")
        .to_owned()
}

#[test]
fn sieve_capability_defaults_match_batch_18_requirements() {
    let server = MockServer::builder().start();
    let session = session(&server);
    let sieve = &session["capabilities"][CAP_SIEVE];

    assert_eq!(sieve["maxSizeScript"], json!(65_536));
    assert_eq!(sieve["maxNumberScripts"], json!(32));
    assert_eq!(sieve["maxSizeScriptName"], json!(512));
    assert_eq!(
        sieve["sieveExtensions"],
        json!(["fileinto", "imap4flags", "copy", "mailboxid", "vacation"])
    );
}

#[test]
fn sieve_capability_overrides_are_advertised() {
    let server = MockServer::builder()
        .sieve_max_size_script(8192)
        .sieve_max_number_scripts(7)
        .sieve_max_size_script_name(64)
        .sieve_extensions(["fileinto", "copy"])
        .start();

    let session = session(&server);
    let sieve = &session["accounts"][ACCOUNT_ID]["accountCapabilities"][CAP_SIEVE];

    assert_eq!(sieve["maxSizeScript"], json!(8192));
    assert_eq!(sieve["maxNumberScripts"], json!(7));
    assert_eq!(sieve["maxSizeScriptName"], json!(64));
    assert_eq!(sieve["sieveExtensions"], json!(["fileinto", "copy"]));
}

#[test]
fn validate_reports_invalid_sieve_with_line_prefix() {
    let server = MockServer::builder().start();
    let blob_id = upload_blob(&server, "application/sieve", b"fileinto \"Inbox\"\n");

    let response = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/validate",
                {
                    "accountId": ACCOUNT_ID,
                    "blobId": blob_id,
                },
                "v1"
            ]],
        }),
    );

    let error = &response["methodResponses"][0][1]["error"];
    assert_eq!(error["type"], "invalidSieve");
    let description = error["description"]
        .as_str()
        .expect("validation error description");
    assert!(description.starts_with("line 1:"), "{description}");
}

#[test]
fn validate_rejects_require_of_unadvertised_extension() {
    let server = MockServer::builder().sieve_extensions(["fileinto"]).start();
    let blob_id = upload_blob(
        &server,
        "application/sieve",
        b"require [\"copy\"];\nkeep;\n",
    );

    let response = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/validate",
                {
                    "accountId": ACCOUNT_ID,
                    "blobId": blob_id,
                },
                "v1"
            ]],
        }),
    );

    let error = &response["methodResponses"][0][1]["error"];
    assert_eq!(error["type"], "invalidSieve");
}

#[test]
fn sieve_set_create_runs_the_same_validation_checker() {
    let server = MockServer::builder().start();
    let blob_id = upload_blob(&server, "application/sieve", b"fileinto \"Inbox\";\n");

    let response = set_create(&server, "script", &blob_id);

    assert_eq!(
        set_error_type(&response, "notCreated", "k1"),
        "invalidSieve"
    );
    let description = response["methodResponses"][0][1]["notCreated"]["k1"]["description"]
        .as_str()
        .expect("set error description");
    assert!(description.starts_with("line 1:"), "{description}");
}

#[test]
fn sieve_limits_return_too_large_and_over_quota_errors() {
    let server = MockServer::builder()
        .sieve_max_size_script(4)
        .sieve_max_number_scripts(1)
        .start();

    let too_large_blob = upload_blob(&server, "application/sieve", b"keep;\n");
    let too_large = set_create(&server, "s1", &too_large_blob);
    assert_eq!(set_error_type(&too_large, "notCreated", "k1"), "tooLarge");

    let ok_blob = upload_blob(&server, "application/sieve", b"x;\n");
    let created = set_create(&server, "s1", &ok_blob);
    let created_id = created["methodResponses"][0][1]["created"]["k1"]["id"]
        .as_str()
        .expect("created script id");
    assert!(!created_id.is_empty());

    let second_blob = upload_blob(&server, "application/sieve", b"y;\n");
    let second = set_create(&server, "s2", &second_blob);
    assert_eq!(set_error_type(&second, "notCreated", "k1"), "overQuota");
}

#[test]
fn sieve_set_update_uses_the_same_checker_as_validate() {
    let server = MockServer::builder().start();
    let ok_blob = upload_blob(&server, "application/sieve", b"keep;\n");
    let created = set_create(&server, "script", &ok_blob);
    let script_id = created["methodResponses"][0][1]["created"]["k1"]["id"]
        .as_str()
        .expect("created script id")
        .to_owned();

    let bad_blob = upload_blob(
        &server,
        "application/sieve",
        b"redirect :copy \"ops@example.test\";\n",
    );
    let response = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/set",
                {
                    "accountId": ACCOUNT_ID,
                    "update": {
                        script_id.clone(): {
                            "blobId": bad_blob,
                        }
                    }
                },
                "s1"
            ]],
        }),
    );

    assert_eq!(
        set_error_type(&response, "notUpdated", &script_id),
        "invalidSieve"
    );
}

#[test]
fn vacation_mode_creates_active_protected_script_on_enable() {
    let server = MockServer::builder().sieve_vacation_script().start();

    let vacation_set = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_VACATION],
            "methodCalls": [[
                "VacationResponse/set",
                {
                    "accountId": ACCOUNT_ID,
                    "update": {
                        "singleton": {
                            "isEnabled": true,
                            "subject": "Out of office",
                            "textBody": "Back soon",
                        }
                    }
                },
                "v1"
            ]],
        }),
    );
    assert!(vacation_set["methodResponses"][0][1]["updated"]["singleton"].is_null());

    let sieve_get = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/get",
                {
                    "accountId": ACCOUNT_ID,
                    "properties": ["id", "name", "isActive"],
                },
                "s1"
            ]],
        }),
    );
    let list = sieve_get["methodResponses"][0][1]["list"]
        .as_array()
        .expect("sieve list");
    let vacation_script = list
        .iter()
        .find(|entry| entry["name"] == "vacation")
        .expect("vacation script entry exists");
    assert_eq!(vacation_script["isActive"], json!(true));

    let script_id = vacation_script["id"]
        .as_str()
        .expect("vacation script id")
        .to_owned();

    let destroy = api(
        &server,
        json!({
            "using": [CAP_CORE, CAP_SIEVE],
            "methodCalls": [[
                "SieveScript/set",
                {
                    "accountId": ACCOUNT_ID,
                    "destroy": [script_id],
                },
                "s2"
            ]],
        }),
    );

    assert_eq!(
        set_error_type(&destroy, "notDestroyed", &script_id),
        "forbidden"
    );
}
