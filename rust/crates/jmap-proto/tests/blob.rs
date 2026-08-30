// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9404 (JMAP Blob Management) unit and roundtrip tests.

use std::collections::BTreeMap;

use jmap_proto::blob::{
    BlobCapability, BlobGetRequest, BlobGetResponse, BlobInfo, BlobUploadRequest,
    BlobUploadResponse, UploadBlob, UploadBlobResult, blob_set_error,
};
use jmap_proto::error::SetError;
use jmap_proto::state::State;
use serde_json::json;

#[test]
fn blob_capability_round_trips() {
    let cap = BlobCapability::new()
        .with_max_size_source(100_000_000)
        .with_max_size_target(50_000_000);

    let val = serde_json::to_value(&cap).expect("to_value");
    assert_eq!(val["maxSizeSource"], 100_000_000);
    assert_eq!(val["maxSizeTarget"], 50_000_000);

    let round: BlobCapability = serde_json::from_value(val).expect("from_value");
    assert_eq!(round, cap);
}

#[test]
fn blob_get_request_and_response_round_trips() {
    let req = BlobGetRequest::new("acc1", vec!["blob1", "blob2"])
        .with_properties(vec!["size", "type"])
        .with_offset(100)
        .with_length(500);

    let val = serde_json::to_value(&req).expect("to_value");
    assert_eq!(val["accountId"], "acc1");
    assert_eq!(val["blobIds"], json!(["blob1", "blob2"]));
    assert_eq!(val["properties"], json!(["size", "type"]));
    assert_eq!(val["offset"], 100);
    assert_eq!(val["length"], 500);

    let round_req: BlobGetRequest = serde_json::from_value(val).expect("from_value");
    assert_eq!(round_req, req);

    let blob_info = BlobInfo::new("blob1", 1024)
        .with_content_type("text/plain")
        .with_data("sample text content");
    let resp = BlobGetResponse::new("acc1", vec![blob_info]).with_not_found(vec!["blob2"]);

    let resp_val = serde_json::to_value(&resp).expect("to_value");
    assert_eq!(resp_val["accountId"], "acc1");
    assert_eq!(resp_val["list"][0]["id"], "blob1");
    assert_eq!(resp_val["list"][0]["size"], 1024);
    assert_eq!(resp_val["list"][0]["type"], "text/plain");
    assert_eq!(resp_val["list"][0]["data"], "sample text content");
    assert_eq!(resp_val["notFound"], json!(["blob2"]));

    let round_resp: BlobGetResponse = serde_json::from_value(resp_val).expect("from_value");
    assert_eq!(round_resp, resp);
}

#[test]
fn blob_upload_request_and_response_round_trips() {
    let upload = UploadBlob::new()
        .with_data("base64data...")
        .with_content_type("application/octet-stream")
        .with_size(1024);

    let req = BlobUploadRequest::new("acc1").create_blob("c1", upload);

    let req_val = serde_json::to_value(&req).expect("to_value");
    assert_eq!(req_val["accountId"], "acc1");
    assert_eq!(req_val["create"]["c1"]["type"], "application/octet-stream");
    assert_eq!(req_val["create"]["c1"]["data"], "base64data...");
    assert_eq!(req_val["create"]["c1"]["size"], 1024);

    let round_req: BlobUploadRequest = serde_json::from_value(req_val).expect("from_value");
    assert_eq!(round_req, req);

    let mut created = BTreeMap::new();
    created.insert(
        "c1".to_string(),
        UploadBlobResult::new("blob_uploaded_1", 1024)
            .with_content_type("application/octet-stream"),
    );
    let mut not_created = BTreeMap::new();
    not_created.insert(
        "c2".to_string(),
        SetError::new(blob_set_error::TOO_LARGE).with_description("blob exceeds quota"),
    );

    let resp = BlobUploadResponse::new("acc1", State::new("st_new"))
        .with_old_state(State::new("st_old"))
        .with_created(created)
        .with_not_created(not_created);

    let resp_val = serde_json::to_value(&resp).expect("to_value");
    assert_eq!(resp_val["accountId"], "acc1");
    assert_eq!(resp_val["oldState"], "st_old");
    assert_eq!(resp_val["newState"], "st_new");
    assert_eq!(resp_val["created"]["c1"]["id"], "blob_uploaded_1");
    assert_eq!(resp_val["notCreated"]["c2"]["type"], "tooLarge");

    let round_resp: BlobUploadResponse = serde_json::from_value(resp_val).expect("from_value");
    assert_eq!(round_resp, resp);
}
