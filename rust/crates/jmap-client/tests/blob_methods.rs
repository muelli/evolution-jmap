// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9404 (JMAP Blob Management) client method tests for `Blob/get`,
//! `Blob/upload`, and `Blob/lookup`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use jmap_client::{Client, Credentials};
use jmap_proto::Id;
use jmap_proto::blob::{
    BlobGetRequest, BlobLookupRequest, BlobUploadRequest, DataSource, UploadBlob,
};
use jmap_proto::session::{CAPABILITY_BLOB, CAPABILITY_CORE};
use serde_json::json;

struct TestServer {
    origin: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TestServer {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind test server");
        let port = server.server_addr().to_ip().expect("server has IP").port();
        let origin = format!("http://127.0.0.1:{port}");
        let stop = Arc::new(AtomicBool::new(false));
        let origin_clone = origin.clone();
        let stop_clone = Arc::clone(&stop);

        let handle = std::thread::spawn(move || {
            while !stop_clone.load(Ordering::SeqCst) {
                match server.recv_timeout(Duration::from_millis(20)) {
                    Ok(Some(mut request)) => {
                        let url = request.url().to_string();
                        if url == "/.well-known/jmap" {
                            let session_doc = json!({
                                "capabilities": {
                                    CAPABILITY_CORE: {},
                                    CAPABILITY_BLOB: {}
                                },
                                "accounts": {
                                    "c": {
                                        "name": "Test User",
                                        "isPersonal": true,
                                        "isReadOnly": false,
                                        "accountCapabilities": {
                                            CAPABILITY_CORE: {},
                                            CAPABILITY_BLOB: {
                                                "maxSizeBlobSet": 7499488,
                                                "maxDataSources": 16,
                                                "supportedTypeNames": ["Email", "Thread", "SieveScript"],
                                                "supportedDigestAlgorithms": ["sha", "sha-256", "sha-512"]
                                            }
                                        }
                                    }
                                },
                                "primaryAccounts": {
                                    CAPABILITY_BLOB: "c"
                                },
                                "username": "user@example.com",
                                "apiUrl": format!("{origin_clone}/jmap/"),
                                "downloadUrl": format!("{origin_clone}/download/{{blobId}}"),
                                "uploadUrl": format!("{origin_clone}/upload/"),
                                "state": "s1"
                            });
                            let data = session_doc.to_string().into_bytes();
                            let response = tiny_http::Response::from_data(data)
                                .with_status_code(200)
                                .with_header(
                                    tiny_http::Header::from_bytes(
                                        &b"Content-Type"[..],
                                        &b"application/json"[..],
                                    )
                                    .unwrap(),
                                );
                            let _ = request.respond(response);
                        } else if url == "/jmap/" {
                            let mut body = Vec::new();
                            let _ = request.as_reader().read_to_end(&mut body);
                            let req_json: serde_json::Value =
                                serde_json::from_slice(&body).expect("valid request JSON");

                            let method_call = &req_json["methodCalls"][0];
                            let method_name = method_call[0].as_str().unwrap();
                            let call_id = method_call[2].as_str().unwrap();

                            let method_response = match method_name {
                                "Blob/upload" => {
                                    json!([
                                        "Blob/upload",
                                        {
                                            "accountId": "c",
                                            "created": {
                                                "k1": {
                                                    "id": "b_up_1",
                                                    "type": "text/plain",
                                                    "size": 18
                                                }
                                            }
                                        },
                                        call_id
                                    ])
                                }
                                "Blob/get" => {
                                    json!([
                                        "Blob/get",
                                        {
                                            "accountId": "c",
                                            "list": [
                                                {
                                                    "id": "b1",
                                                    "data:asText": "hello world",
                                                    "data:asBase64": "aGVsbG8gd29ybGQ=",
                                                    "size": 11,
                                                    "digest:sha-256": "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
                                                }
                                            ],
                                            "notFound": ["b_missing"]
                                        },
                                        call_id
                                    ])
                                }
                                "Blob/lookup" => {
                                    json!([
                                        "Blob/lookup",
                                        {
                                            "accountId": "c",
                                            "list": [
                                                {
                                                    "id": "b1",
                                                    "matchedIds": {
                                                        "Email": ["M1"]
                                                    }
                                                }
                                            ],
                                            "notFound": ["b_missing"]
                                        },
                                        call_id
                                    ])
                                }
                                other => panic!("unexpected method: {other}"),
                            };

                            let resp_doc = json!({
                                "latestClientVersion": null,
                                "methodResponses": [method_response],
                                "sessionState": "s1"
                            });
                            let data = resp_doc.to_string().into_bytes();
                            let response = tiny_http::Response::from_data(data)
                                .with_status_code(200)
                                .with_header(
                                    tiny_http::Header::from_bytes(
                                        &b"Content-Type"[..],
                                        &b"application/json"[..],
                                    )
                                    .unwrap(),
                                );
                            let _ = request.respond(response);
                        } else {
                            let response =
                                tiny_http::Response::from_string("Not Found").with_status_code(404);
                            let _ = request.respond(response);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        });

        Self {
            origin,
            stop,
            handle: Some(handle),
        }
    }

    fn origin(&self) -> &str {
        &self.origin
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn account_blob_capability_is_accessible() {
    let server = TestServer::start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connect");

    let account_id = Id::from("c");
    let account = client
        .session()
        .accounts
        .get(&account_id)
        .expect("account c exists");
    let blob_cap = account.blob_capability().expect("has blob capability");
    assert_eq!(blob_cap.max_size_blob_set, Some(7499488));
    assert_eq!(blob_cap.max_data_sources, Some(16));
    assert_eq!(
        blob_cap.supported_type_names.as_deref(),
        Some(
            &[
                "Email".to_string(),
                "Thread".to_string(),
                "SieveScript".to_string()
            ][..]
        )
    );
    assert_eq!(
        blob_cap.supported_digest_algorithms.as_deref(),
        Some(
            &[
                "sha".to_string(),
                "sha-256".to_string(),
                "sha-512".to_string()
            ][..]
        )
    );
}

#[test]
fn blob_upload_method_call_and_response() {
    let server = TestServer::start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connect");

    let upload = UploadBlob::new()
        .with_data([DataSource::as_text("hello "), DataSource::as_text("world!")])
        .with_content_type("text/plain");
    let req = BlobUploadRequest::new("c").create_blob("k1", upload);

    let resp = client.blob_upload(&req).expect("blob_upload succeeds");
    assert_eq!(resp.account_id, Id::from("c"));
    let created = resp.created.expect("created map present");
    let result = created.get("k1").expect("k1 created");
    assert_eq!(result.id, Id::from("b_up_1"));
    assert_eq!(result.content_type.as_deref(), Some("text/plain"));
    assert_eq!(result.size, 18);
}

#[test]
fn blob_get_method_call_and_response() {
    let server = TestServer::start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connect");

    let req = BlobGetRequest::new("c", ["b1", "b_missing"]).with_properties([
        "id",
        "data:asText",
        "data:asBase64",
        "size",
        "digest:sha-256",
    ]);

    let resp = client.blob_get(&req).expect("blob_get succeeds");
    assert_eq!(resp.account_id, Id::from("c"));
    assert_eq!(resp.list.len(), 1);
    let blob = &resp.list[0];
    assert_eq!(blob.id, Id::from("b1"));
    assert_eq!(blob.data_as_text.as_deref(), Some("hello world"));
    assert_eq!(blob.data_as_base64.as_deref(), Some("aGVsbG8gd29ybGQ="));
    assert_eq!(blob.size, Some(11));
    assert_eq!(
        blob.digest("sha-256"),
        Some("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
    );
    assert_eq!(resp.not_found, vec![Id::from("b_missing")]);
}

#[test]
fn blob_lookup_method_call_and_response() {
    let server = TestServer::start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connect");

    let req = BlobLookupRequest::new("c", ["Email"], ["b1", "b_missing"]);

    let resp = client.blob_lookup(&req).expect("blob_lookup succeeds");
    assert_eq!(resp.account_id, Id::from("c"));
    assert_eq!(resp.list.len(), 1);
    let item = &resp.list[0];
    assert_eq!(item.id, Id::from("b1"));
    assert_eq!(item.matched_ids.get("Email"), Some(&vec![Id::from("M1")]));
    assert_eq!(resp.not_found, vec![Id::from("b_missing")]);
}
