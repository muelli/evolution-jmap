// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify `jmap-mockd --hostile <scenario>` across standalone CLI invocations.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::Command;

use serde_json::json;

mod common;
use common::Mockd;

#[test]
fn help_lists_hostile_flag_and_scenarios() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(
        stderr.contains("--hostile"),
        "missing --hostile in help output:\n{stderr}"
    );
}

#[test]
fn mockd_hostile_oversized_response() {
    let mockd = Mockd::spawn(&["--hostile", "oversized-response"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Core/echo", {}, "c1"]]
    });
    let payload = serde_json::to_vec(&req).unwrap();
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    assert!(
        body.len() > 10_000_000,
        "body must exceed 10MB, got {} bytes",
        body.len()
    );
}

#[test]
fn mockd_hostile_deep_json() {
    let mockd = Mockd::spawn(&["--hostile", "deep-json"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Core/echo", {}, "c1"]]
    });
    let payload = serde_json::to_vec(&req).unwrap();
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    let open_brackets = body.chars().filter(|c| *c == '[').count();
    assert!(
        open_brackets >= 10_000,
        "expected >= 10000 open brackets, got {open_brackets}"
    );
}

#[test]
fn mockd_hostile_numbers_out_of_range() {
    let mockd = Mockd::spawn(&["--hostile", "numbers-out-of-range"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Email/query", {"accountId": "A1"}, "c1"]]
    });
    let payload = serde_json::to_vec(&req).unwrap();
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    assert!(
        body.contains("99999999999999999999999999999999"),
        "expected huge number, got {body}"
    );
    assert!(
        body.contains("-1") || body.contains("-42"),
        "expected negative count, got {body}"
    );
}

#[test]
fn mockd_hostile_dangerous_ids() {
    let mockd = Mockd::spawn(&["--hostile", "dangerous-ids"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Email/query", {"accountId": "A1"}, "c1"]]
    });
    let payload = serde_json::to_vec(&req).unwrap();
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    assert!(
        body.contains("../../etc/passwd") || body.contains("../evil/path"),
        "expected traversal path in body: {body}"
    );
}

#[test]
fn mockd_hostile_blob_size_mismatch() {
    let mockd = Mockd::spawn(&["--hostile", "blob-size-mismatch"]);
    let (status, _, body) = mockd.request("POST", "/upload/A1", b"short data");
    assert!(status.contains("201"), "{status}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    let declared_size = parsed["size"].as_u64().unwrap();
    assert_eq!(declared_size, 1_000_000);

    let blob_id = parsed["blobId"].as_str().unwrap();
    let download_path = format!("/download/A1/{blob_id}/test.bin");
    let (status, _, data) = mockd.request("GET", &download_path, b"");
    assert!(status.contains("200"), "{status}");
    assert_ne!(data.len() as u64, declared_size);
}

#[test]
fn mockd_hostile_session_urls_other_origin() {
    let mockd = Mockd::spawn(&["--hostile", "session-urls-other-origin"]);
    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");
    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).unwrap();
    let api_url = session["apiUrl"].as_str().unwrap();
    assert!(
        api_url.starts_with("https://evil.example.com"),
        "apiUrl: {api_url}"
    );
}

#[test]
fn mockd_hostile_session_urls_file() {
    let mockd = Mockd::spawn(&["--hostile", "session-urls-file"]);
    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");
    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).unwrap();
    let api_url = session["apiUrl"].as_str().unwrap();
    assert!(api_url.starts_with("file://"), "apiUrl: {api_url}");
}

#[test]
fn mockd_hostile_redirect_other_origin() {
    let mockd = Mockd::spawn(&["--hostile", "redirect-other-origin"]);
    let (status, headers, _) = mockd.request("POST", "/jmap", b"{}");
    assert!(
        status.contains("307") || status.contains("302"),
        "expected redirect, got: {status}"
    );
    let location = headers
        .iter()
        .find(|h| h.to_lowercase().starts_with("location:"))
        .expect("location header");
    assert!(
        location.contains("evil.example.com"),
        "location: {location}"
    );
}

#[test]
fn mockd_hostile_wrong_method_call_id() {
    let mockd = Mockd::spawn(&["--hostile", "wrong-method-call-id"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Core/echo", {}, "call_alpha"]]
    });
    let payload = serde_json::to_vec(&req).unwrap();
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    let call_id = resp["methodResponses"][0][2].as_str().unwrap();
    assert_ne!(call_id, "call_alpha");
}

#[test]
fn mockd_hostile_duplicate_method_call_id() {
    let mockd = Mockd::spawn(&["--hostile", "duplicate-method-call-id"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {"x": 1}, "c1"],
            ["Core/echo", {"y": 2}, "c2"]
        ]
    });
    let payload = serde_json::to_vec(&req).unwrap();
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    let calls = resp["methodResponses"].as_array().unwrap();
    assert_eq!(calls[0][2], calls[1][2]);
}

#[test]
fn mockd_hostile_out_of_order_responses() {
    let mockd = Mockd::spawn(&["--hostile", "out-of-order-responses"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {"n": 1}, "c1"],
            ["Core/echo", {"n": 2}, "c2"],
            ["Core/echo", {"n": 3}, "c3"]
        ]
    });
    let payload = serde_json::to_vec(&req).unwrap();
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    let resp: serde_json::Value = serde_json::from_str(&body).unwrap();
    let calls = resp["methodResponses"].as_array().unwrap();
    let ids: Vec<&str> = calls.iter().map(|c| c[2].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["c3", "c2", "c1"]);
}

#[test]
fn mockd_hostile_replayed_response() {
    let mockd = Mockd::spawn(&["--hostile", "replayed-response"]);
    let req1 = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Core/echo", {"req": 1}, "c1"]]
    });
    let (_, _, body1) = mockd.request("POST", "/jmap", &serde_json::to_vec(&req1).unwrap());

    let req2 = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Core/echo", {"req": 2}, "c2"]]
    });
    let (_, _, body2) = mockd.request("POST", "/jmap", &serde_json::to_vec(&req2).unwrap());

    assert_eq!(body1, body2);
}

#[test]
fn mockd_hostile_state_backwards() {
    let mockd = Mockd::spawn(&["--hostile", "state-backwards"]);
    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [["Core/echo", {}, "c1"]]
    });
    let payload = serde_json::to_vec(&req).unwrap();

    let (_, _, body1) = mockd.request("POST", "/jmap", &payload);
    let resp1: serde_json::Value = serde_json::from_str(&body1).unwrap();
    let s1: u64 = resp1["sessionState"].as_str().unwrap().parse().unwrap();

    let (_, _, body2) = mockd.request("POST", "/jmap", &payload);
    let resp2: serde_json::Value = serde_json::from_str(&body2).unwrap();
    let s2: u64 = resp2["sessionState"].as_str().unwrap().parse().unwrap();

    assert!(s2 < s1, "expected state regression: s1={s1}, s2={s2}");
}

#[test]
fn mockd_hostile_eventsource_unknown() {
    let mockd = Mockd::spawn(&["--hostile", "eventsource-unknown"]);
    let address = mockd.origin.strip_prefix("http://").expect("plain HTTP");
    let mut stream = TcpStream::connect(address).expect("connect to mock");
    let head = format!(
        "GET /eventsource?closeafter=state HTTP/1.1\r\n\
         Host: {address}\r\n\
         Connection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).expect("write request");
    stream.flush().expect("flush request");

    let reader = BufReader::new(stream);
    let mut found = false;
    for line in reader.lines() {
        let line = line.expect("readable SSE line");
        if line.contains("unknown-account-XYZ") || line.contains("UnknownType123") {
            found = true;
            break;
        }
    }
    assert!(found, "eventsource must push unknown account/type");
}
