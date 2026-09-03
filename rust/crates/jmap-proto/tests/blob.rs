// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9404 (JMAP Blob Management) unit, roundtrip, and capability tests.

use std::collections::BTreeMap;

use jmap_proto::blob::{
    BlobCapability, BlobGetRequest, BlobGetResponse, BlobInfo, BlobLookupMatch, BlobLookupRequest,
    BlobLookupResponse, BlobUploadRequest, BlobUploadResponse, DataSource, UploadBlob,
    UploadBlobResult, blob_set_error,
};
use jmap_proto::error::SetError;
use jmap_proto::session::Session;
use serde_json::json;

#[test]
fn blob_account_capability_matches_stalwart_probe_and_rfc9404() {
    let raw = json!({
        "maxSizeBlobSet": 7499488,
        "maxDataSources": 16,
        "supportedTypeNames": ["Email", "Thread", "SieveScript"],
        "supportedDigestAlgorithms": ["sha", "sha-256", "sha-512"]
    });

    let cap: BlobCapability = serde_json::from_value(raw).expect("deserializes BlobCapability");
    assert_eq!(cap.max_size_blob_set, Some(7499488));
    assert_eq!(cap.max_data_sources, Some(16));
    assert_eq!(
        cap.supported_type_names.as_deref(),
        Some(
            &[
                "Email".to_string(),
                "Thread".to_string(),
                "SieveScript".to_string()
            ][..]
        )
    );
    assert_eq!(
        cap.supported_digest_algorithms.as_deref(),
        Some(
            &[
                "sha".to_string(),
                "sha-256".to_string(),
                "sha-512".to_string()
            ][..]
        )
    );

    // Fluent builder round-trip
    let built = BlobCapability::new()
        .with_max_size_blob_set(7499488)
        .with_max_data_sources(16)
        .with_supported_type_names(["Email", "Thread", "SieveScript"])
        .with_supported_digest_algorithms(["sha", "sha-256", "sha-512"]);

    let val = serde_json::to_value(&built).expect("serializes");
    assert_eq!(val["maxSizeBlobSet"], 7499488);
    assert_eq!(val["maxDataSources"], 16);
    assert_eq!(
        val["supportedTypeNames"],
        json!(["Email", "Thread", "SieveScript"])
    );
    assert_eq!(
        val["supportedDigestAlgorithms"],
        json!(["sha", "sha-256", "sha-512"])
    );

    let round: BlobCapability = serde_json::from_value(val).expect("roundtrips");
    assert_eq!(round, built);
}

#[test]
fn session_and_account_blob_capability_resolution() {
    let raw = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {},
            "urn:ietf:params:jmap:blob": {}
        },
        "accounts": {
            "c": {
                "name": "User Account",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:blob": {
                        "maxSizeBlobSet": 7499488,
                        "maxDataSources": 16,
                        "supportedTypeNames": ["Email", "Thread", "SieveScript"],
                        "supportedDigestAlgorithms": ["sha", "sha-256", "sha-512"]
                    }
                }
            }
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:blob": "c"
        },
        "username": "user@example.com",
        "apiUrl": "http://127.0.0.1:8080/jmap/",
        "downloadUrl": "http://127.0.0.1:8080/download/{blobId}",
        "uploadUrl": "http://127.0.0.1:8080/upload/",
        "state": "s1"
    });

    let session: Session = serde_json::from_value(raw).expect("session deserializes");
    assert!(session.blob_capability().is_some());

    let account_id = jmap_proto::Id::from("c");
    let account = session.accounts.get(&account_id).expect("account c exists");
    let acct_cap = account
        .blob_capability()
        .expect("has blob capability on account");
    assert_eq!(acct_cap.max_size_blob_set, Some(7499488));
    assert_eq!(acct_cap.max_data_sources, Some(16));
    assert_eq!(
        acct_cap.supported_type_names.as_deref(),
        Some(
            &[
                "Email".to_string(),
                "Thread".to_string(),
                "SieveScript".to_string()
            ][..]
        )
    );
}

#[test]
fn data_source_shapes_and_builders() {
    let text_ds = DataSource::as_text("hello world");
    let val_text = serde_json::to_value(&text_ds).expect("serializes text ds");
    assert_eq!(val_text["data:asText"], "hello world");
    assert!(val_text.get("data:asBase64").is_none());
    assert!(val_text.get("blobId").is_none());

    let b64_ds = DataSource::as_base64("aGVsbG8=");
    let val_b64 = serde_json::to_value(&b64_ds).expect("serializes base64 ds");
    assert_eq!(val_b64["data:asBase64"], "aGVsbG8=");

    let blob_ds = DataSource::from_blob_id("b1")
        .with_offset(10)
        .with_length(20);
    let val_blob = serde_json::to_value(&blob_ds).expect("serializes blobId ds");
    assert_eq!(val_blob["blobId"], "b1");
    assert_eq!(val_blob["offset"], 10);
    assert_eq!(val_blob["length"], 20);
}

#[test]
fn blob_upload_request_and_response_round_trips() {
    let upload = UploadBlob::new()
        .with_data([
            DataSource::as_text("part 1"),
            DataSource::as_base64("cGFydCAy"),
            DataSource::from_blob_id("b_existing"),
        ])
        .with_content_type("text/plain");

    let req = BlobUploadRequest::new("c").create_blob("k1", upload);

    let val = serde_json::to_value(&req).expect("serializes req");
    assert_eq!(val["accountId"], "c");
    assert_eq!(val["create"]["k1"]["type"], "text/plain");
    assert_eq!(val["create"]["k1"]["data"][0]["data:asText"], "part 1");
    assert_eq!(val["create"]["k1"]["data"][1]["data:asBase64"], "cGFydCAy");
    assert_eq!(val["create"]["k1"]["data"][2]["blobId"], "b_existing");

    let round_req: BlobUploadRequest = serde_json::from_value(val).expect("roundtrips req");
    assert_eq!(round_req, req);

    let mut created = BTreeMap::new();
    created.insert(
        "k1".to_string(),
        UploadBlobResult::new("b_new_1", 1024).with_content_type("text/plain"),
    );
    let mut not_created = BTreeMap::new();
    not_created.insert(
        "k2".to_string(),
        SetError::new(blob_set_error::TOO_LARGE).with_description("blob too large"),
    );

    let resp = BlobUploadResponse::new("c")
        .with_created(created)
        .with_not_created(not_created);

    let resp_val = serde_json::to_value(&resp).expect("serializes resp");
    assert_eq!(resp_val["accountId"], "c");
    assert_eq!(resp_val["created"]["k1"]["id"], "b_new_1");
    assert_eq!(resp_val["created"]["k1"]["type"], "text/plain");
    assert_eq!(resp_val["created"]["k1"]["size"], 1024);
    assert_eq!(resp_val["notCreated"]["k2"]["type"], "tooLarge");

    let round_resp: BlobUploadResponse = serde_json::from_value(resp_val).expect("roundtrips resp");
    assert_eq!(round_resp, resp);
}

#[test]
fn blob_get_request_and_response_with_dynamic_properties() {
    let req = BlobGetRequest::new("c", ["b1", "b2"])
        .with_properties([
            "id",
            "data:asText",
            "data:asBase64",
            "size",
            "digest:sha-256",
        ])
        .with_offset(0)
        .with_length(512);

    let val = serde_json::to_value(&req).expect("serializes req");
    assert_eq!(val["accountId"], "c");
    assert_eq!(val["ids"], json!(["b1", "b2"]));
    assert_eq!(
        val["properties"],
        json!([
            "id",
            "data:asText",
            "data:asBase64",
            "size",
            "digest:sha-256"
        ])
    );
    assert_eq!(val["offset"], 0);
    assert_eq!(val["length"], 512);

    let round_req: BlobGetRequest = serde_json::from_value(val).expect("roundtrips req");
    assert_eq!(round_req, req);

    let item = BlobInfo::from_id("b1")
        .with_data_as_text("hello world")
        .with_data_as_base64("aGVsbG8gd29ybGQ=")
        .with_size(11)
        .with_digest(
            "sha-256",
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9",
        );

    assert_eq!(
        item.digest("sha-256"),
        Some("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
    );
    assert_eq!(item.digest("sha"), None);

    let resp = BlobGetResponse::new("c", [item]).with_not_found(["b2"]);
    let resp_val = serde_json::to_value(&resp).expect("serializes resp");

    assert_eq!(resp_val["accountId"], "c");
    assert_eq!(resp_val["list"][0]["id"], "b1");
    assert_eq!(resp_val["list"][0]["data:asText"], "hello world");
    assert_eq!(resp_val["list"][0]["data:asBase64"], "aGVsbG8gd29ybGQ=");
    assert_eq!(resp_val["list"][0]["size"], 11);
    assert_eq!(
        resp_val["list"][0]["digest:sha-256"],
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
    assert_eq!(resp_val["notFound"], json!(["b2"]));

    let round_resp: BlobGetResponse = serde_json::from_value(resp_val).expect("roundtrips resp");
    assert_eq!(round_resp, resp);
}

#[test]
fn blob_lookup_request_and_response_round_trips() {
    let req = BlobLookupRequest::new("c", ["Email", "Thread"], ["b1", "b2"]);
    let val = serde_json::to_value(&req).expect("serializes req");
    assert_eq!(val["accountId"], "c");
    assert_eq!(val["typeNames"], json!(["Email", "Thread"]));
    assert_eq!(val["ids"], json!(["b1", "b2"]));

    let round_req: BlobLookupRequest = serde_json::from_value(val).expect("roundtrips req");
    assert_eq!(round_req, req);

    let match_item = BlobLookupMatch::new("b1")
        .with_type_matched_ids("Email", ["M1", "M2"])
        .with_type_matched_ids("Thread", ["T1"]);

    let resp = BlobLookupResponse::new("c", [match_item]).with_not_found(["b2"]);
    let resp_val = serde_json::to_value(&resp).expect("serializes resp");

    assert_eq!(resp_val["accountId"], "c");
    assert_eq!(resp_val["list"][0]["id"], "b1");
    assert_eq!(
        resp_val["list"][0]["matchedIds"]["Email"],
        json!(["M1", "M2"])
    );
    assert_eq!(resp_val["list"][0]["matchedIds"]["Thread"], json!(["T1"]));
    assert_eq!(resp_val["notFound"], json!(["b2"]));

    let round_resp: BlobLookupResponse = serde_json::from_value(resp_val).expect("roundtrips resp");
    assert_eq!(round_resp, resp);
}

#[test]
fn blob_set_error_constants() {
    assert_eq!(blob_set_error::BLOB_NOT_FOUND, "blobNotFound");
    assert_eq!(blob_set_error::TOO_LARGE, "tooLarge");
    assert_eq!(blob_set_error::MAX_DATA_SOURCES, "maxDataSources");
}
