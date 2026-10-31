// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for hostile server scenarios configured via builder.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use jmap_mock::{HostileScenario, MockServer};
use serde_json::json;

fn http_post(origin: &str, path: &str, body: &[u8]) -> (String, Vec<String>, Vec<u8>) {
    let address = origin
        .strip_prefix("http://")
        .expect("mock serves plain HTTP");
    let mut stream = TcpStream::connect(address).expect("connect to mock");

    let head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );

    stream.write_all(head.as_bytes()).expect("write header");
    if !body.is_empty() {
        stream.write_all(body).expect("write body");
    }
    stream.flush().expect("flush request");

    let mut response_bytes = Vec::new();
    stream
        .read_to_end(&mut response_bytes)
        .expect("read response");

    let header_end = response_bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header delimiter");
    let header_str = String::from_utf8_lossy(&response_bytes[..header_end]);
    let mut lines = header_str.lines();
    let status = lines.next().expect("status line").to_owned();
    let headers: Vec<String> = lines.map(str::to_owned).collect();
    let body = response_bytes[header_end + 4..].to_vec();

    (status, headers, body)
}

fn http_get(origin: &str, path: &str) -> (String, Vec<String>, Vec<u8>) {
    let address = origin
        .strip_prefix("http://")
        .expect("mock serves plain HTTP");
    let mut stream = TcpStream::connect(address).expect("connect to mock");

    let head = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Connection: close\r\n\r\n"
    );

    stream.write_all(head.as_bytes()).expect("write header");
    stream.flush().expect("flush request");

    let mut response_bytes = Vec::new();
    stream
        .read_to_end(&mut response_bytes)
        .expect("read response");

    let header_end = response_bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header delimiter");
    let header_str = String::from_utf8_lossy(&response_bytes[..header_end]);
    let mut lines = header_str.lines();
    let status = lines.next().expect("status line").to_owned();
    let headers: Vec<String> = lines.map(str::to_owned).collect();
    let body = response_bytes[header_end + 4..].to_vec();

    (status, headers, body)
}

#[test]
fn hostile_scenario_oversized_response() {
    let server = MockServer::builder()
        .hostile(HostileScenario::OversizedResponse)
        .start();

    let request = json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        "methodCalls": [
            ["Mailbox/get", {"accountId": "A1"}, "c1"]
        ]
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let (status, _, body) = http_post(server.origin(), "/jmap", &payload);

    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    assert!(
        body.len() > 10_000_000,
        "oversized response must exceed 10MB, got {} bytes",
        body.len()
    );
}

#[test]
fn hostile_scenario_deep_json() {
    let server = MockServer::builder()
        .hostile(HostileScenario::DeepJson)
        .start();

    let request = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {}, "c1"]
        ]
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let (status, _, body) = http_post(server.origin(), "/jmap", &payload);

    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    let text = String::from_utf8_lossy(&body);
    let open_brackets = text.chars().filter(|c| *c == '[').count();
    let close_brackets = text.chars().filter(|c| *c == ']').count();
    assert!(
        open_brackets >= 10_000,
        "expected at least 10000 open brackets, found {open_brackets}"
    );
    assert!(
        close_brackets >= 10_000,
        "expected at least 10000 close brackets, found {close_brackets}"
    );
}

#[test]
fn hostile_scenario_numbers_out_of_range() {
    let server = MockServer::builder()
        .hostile(HostileScenario::NumbersOutOfRange)
        .start();

    let request = json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        "methodCalls": [
            ["Email/query", {"accountId": "A1"}, "c1"]
        ]
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let (status, _, body) = http_post(server.origin(), "/jmap", &payload);

    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("99999999999999999999999999999999"),
        "expected number beyond 2^53, got: {text}"
    );
    assert!(
        text.contains("-1") || text.contains("-42"),
        "expected negative count in response, got: {text}"
    );
}

#[test]
fn hostile_scenario_dangerous_ids() {
    let server = MockServer::builder()
        .hostile(HostileScenario::DangerousIds)
        .start();

    let request = json!({
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        "methodCalls": [
            ["Email/query", {"accountId": "A1"}, "c1"]
        ]
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let (status, _, body) = http_post(server.origin(), "/jmap", &payload);

    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("../../etc/passwd") || text.contains("../evil/path"),
        "expected path traversal characters in ids, got: {text}"
    );
    assert!(
        text.contains("id/with/slash"),
        "expected slash in id, got: {text}"
    );
    assert!(
        text.contains(r"\u0000") || text.contains('\0'),
        "expected NUL in id, got: {text}"
    );
}

#[test]
fn hostile_scenario_blob_size_mismatch() {
    let server = MockServer::builder()
        .hostile(HostileScenario::BlobSizeMismatch)
        .start();

    let upload_body = b"hello world";
    let (status, _, body) = http_post(server.origin(), "/upload/A1", upload_body);
    assert!(
        status.contains("201"),
        "expected 201 Created, got: {status}"
    );

    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let blob_id = parsed["blobId"].as_str().expect("blobId");
    let declared_size = parsed["size"].as_u64().expect("declared size");
    assert_eq!(declared_size, 1_000_000, "declared size should be 1000000");

    let download_path = format!("/download/A1/{blob_id}/test.bin");
    let (status, _, data) = http_get(server.origin(), &download_path);
    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    assert_ne!(
        data.len() as u64,
        declared_size,
        "served size must differ from declared size"
    );
}

#[test]
fn hostile_scenario_session_urls_other_origin() {
    let server = MockServer::builder()
        .hostile(HostileScenario::SessionUrlsOtherOrigin)
        .start();

    let (status, _, body) = http_get(server.origin(), "/.well-known/jmap");
    assert!(status.contains("200"), "expected 200 OK, got: {status}");

    let session: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let api_url = session["apiUrl"].as_str().expect("apiUrl");
    let download_url = session["downloadUrl"].as_str().expect("downloadUrl");
    let upload_url = session["uploadUrl"].as_str().expect("uploadUrl");

    assert!(
        api_url.starts_with("https://evil.example.com"),
        "apiUrl must point to foreign origin: {api_url}"
    );
    assert!(
        download_url.starts_with("https://evil.example.com"),
        "downloadUrl must point to foreign origin: {download_url}"
    );
    assert!(
        upload_url.starts_with("https://evil.example.com"),
        "uploadUrl must point to foreign origin: {upload_url}"
    );
}

#[test]
fn hostile_scenario_session_urls_file() {
    let server = MockServer::builder()
        .hostile(HostileScenario::SessionUrlsFile)
        .start();

    let (status, _, body) = http_get(server.origin(), "/.well-known/jmap");
    assert!(status.contains("200"), "expected 200 OK, got: {status}");

    let session: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let api_url = session["apiUrl"].as_str().expect("apiUrl");
    let download_url = session["downloadUrl"].as_str().expect("downloadUrl");
    let upload_url = session["uploadUrl"].as_str().expect("uploadUrl");

    assert!(
        api_url.starts_with("file://"),
        "apiUrl must point to file scheme: {api_url}"
    );
    assert!(
        download_url.starts_with("file://"),
        "downloadUrl must point to file scheme: {download_url}"
    );
    assert!(
        upload_url.starts_with("file://"),
        "uploadUrl must point to file scheme: {upload_url}"
    );
}

#[test]
fn hostile_scenario_redirect_other_origin() {
    let server = MockServer::builder()
        .hostile(HostileScenario::RedirectOtherOrigin)
        .start();

    let (status, headers, _) = http_post(server.origin(), "/jmap", b"{}");
    assert!(
        status.contains("307") || status.contains("302"),
        "expected redirect status, got: {status}"
    );
    let location = headers
        .iter()
        .find(|h| h.to_lowercase().starts_with("location:"))
        .expect("location header");
    assert!(
        location.contains("evil.example.com"),
        "location must point to other origin: {location}"
    );
}

#[test]
fn hostile_scenario_wrong_method_call_id() {
    let server = MockServer::builder()
        .hostile(HostileScenario::WrongMethodCallId)
        .start();

    let request = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {}, "client_call_id_42"]
        ]
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let (status, _, body) = http_post(server.origin(), "/jmap", &payload);

    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let call_id = response["methodResponses"][0][2].as_str().expect("call id");
    assert_ne!(
        call_id, "client_call_id_42",
        "mock must return a wrong method call id"
    );
}

#[test]
fn hostile_scenario_duplicate_method_call_id() {
    let server = MockServer::builder()
        .hostile(HostileScenario::DuplicateMethodCallId)
        .start();

    let request = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {"a": 1}, "call_1"],
            ["Core/echo", {"b": 2}, "call_2"]
        ]
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let (status, _, body) = http_post(server.origin(), "/jmap", &payload);

    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let calls = response["methodResponses"].as_array().expect("array");
    assert!(calls.len() >= 2, "expected at least 2 responses");
    let id0 = calls[0][2].as_str().unwrap();
    let id1 = calls[1][2].as_str().unwrap();
    assert_eq!(
        id0, id1,
        "method responses must share a duplicate call id, got {id0} and {id1}"
    );
}

#[test]
fn hostile_scenario_out_of_order_responses() {
    let server = MockServer::builder()
        .hostile(HostileScenario::OutOfOrderResponses)
        .start();

    let request = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {"order": 1}, "c1"],
            ["Core/echo", {"order": 2}, "c2"],
            ["Core/echo", {"order": 3}, "c3"]
        ]
    });
    let payload = serde_json::to_vec(&request).unwrap();
    let (status, _, body) = http_post(server.origin(), "/jmap", &payload);

    assert!(status.contains("200"), "expected 200 OK, got: {status}");
    let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let calls = response["methodResponses"].as_array().expect("array");
    assert_eq!(calls.len(), 3);
    let ids: Vec<&str> = calls.iter().map(|c| c[2].as_str().unwrap()).collect();
    assert_ne!(
        ids,
        vec!["c1", "c2", "c3"],
        "responses must not match request order"
    );
    assert_eq!(ids, vec!["c3", "c2", "c1"], "responses should be reversed");
}

#[test]
fn hostile_scenario_replayed_response() {
    let server = MockServer::builder()
        .hostile(HostileScenario::ReplayedResponse)
        .start();

    let req1 = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {"tag": "first"}, "call_first"]
        ]
    });
    let (status1, _, body1) = http_post(
        server.origin(),
        "/jmap",
        &serde_json::to_vec(&req1).unwrap(),
    );
    assert!(status1.contains("200"), "expected 200 OK, got: {status1}");

    let req2 = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {"tag": "second"}, "call_second"]
        ]
    });
    let (status2, _, body2) = http_post(
        server.origin(),
        "/jmap",
        &serde_json::to_vec(&req2).unwrap(),
    );
    assert!(status2.contains("200"), "expected 200 OK, got: {status2}");

    assert_eq!(
        body1, body2,
        "second request must return the exact replayed response from the first"
    );
}

#[test]
fn hostile_scenario_state_backwards() {
    let server = MockServer::builder()
        .hostile(HostileScenario::StateBackwards)
        .start();

    let req = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": [
            ["Core/echo", {}, "c1"]
        ]
    });
    let payload = serde_json::to_vec(&req).unwrap();

    let (_, _, body1) = http_post(server.origin(), "/jmap", &payload);
    let resp1: serde_json::Value = serde_json::from_slice(&body1).unwrap();
    let state1: u64 = resp1["sessionState"].as_str().unwrap().parse().unwrap();

    let (_, _, body2) = http_post(server.origin(), "/jmap", &payload);
    let resp2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    let state2: u64 = resp2["sessionState"].as_str().unwrap().parse().unwrap();

    assert!(
        state2 < state1,
        "state must go backwards: state1={state1}, state2={state2}"
    );
}

#[test]
fn hostile_scenario_eventsource_unknown() {
    let server = MockServer::builder()
        .hostile(HostileScenario::EventsourceUnknown)
        .start();

    let address = server.origin().strip_prefix("http://").expect("plain HTTP");
    let mut stream = TcpStream::connect(address).expect("connect to mock");
    let head = format!(
        "GET /eventsource?closeafter=state HTTP/1.1\r\n\
         Host: {address}\r\n\
         Connection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).expect("write request");
    stream.flush().expect("flush request");

    let reader = BufReader::new(stream);
    let mut found_unknown = false;
    for line in reader.lines() {
        let line = line.expect("readable SSE line");
        if line.contains("unknown-account-XYZ") || line.contains("UnknownType123") {
            found_unknown = true;
            break;
        }
    }

    assert!(
        found_unknown,
        "eventsource must emit event with unknown account and unknown type"
    );
}
