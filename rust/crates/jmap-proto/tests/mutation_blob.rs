// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::blob::{
    BlobCapability, BlobGetRequest, BlobInfo, BlobLookupMatch, BlobLookupResponse,
    BlobUploadResponse, DataSource, UploadBlob, UploadBlobResult,
};
use jmap_proto::id::Id;
use jmap_proto::request::ResultReference;
use serde_json::json;

#[test]
fn blob_capability_size_limits_and_extra_builders() {
    let mut extra = BTreeMap::new();
    extra.insert("vendorBlobQuirks".to_string(), json!(true));

    let cap = BlobCapability::new()
        .with_max_size_blob_set(100_000_000)
        .with_max_data_sources(32)
        .with_max_size_source(50_000_000)
        .with_max_size_target(25_000_000)
        .with_supported_type_names(["Email", "Blob"])
        .with_supported_digest_algorithms(["sha-256"])
        .with_extra(extra);

    assert_eq!(cap.max_size_blob_set, Some(100_000_000));
    assert_eq!(cap.max_data_sources, Some(32));
    assert_eq!(cap.max_size_source, Some(50_000_000));
    assert_eq!(cap.max_size_target, Some(25_000_000));
    assert_eq!(
        cap.supported_type_names.as_deref(),
        Some(&["Email".to_string(), "Blob".to_string()][..])
    );
    assert_eq!(
        cap.supported_digest_algorithms.as_deref(),
        Some(&["sha-256".to_string()][..])
    );
    assert_eq!(cap.extra["vendorBlobQuirks"], json!(true));

    let val = serde_json::to_value(&cap).expect("serializes BlobCapability");
    assert_eq!(val["maxSizeSource"], 50_000_000);
    assert_eq!(val["maxSizeTarget"], 25_000_000);
    assert_eq!(val["vendorBlobQuirks"], true);

    let round: BlobCapability = serde_json::from_value(val).expect("deserializes BlobCapability");
    assert_eq!(round.max_size_source, Some(50_000_000));
    assert_eq!(round.max_size_target, Some(25_000_000));
    assert_eq!(round.extra["vendorBlobQuirks"], json!(true));
}

#[test]
fn upload_blob_from_text_and_base64_constructors() {
    let from_text = UploadBlob::from_text("sample plain text", "text/plain");
    assert_eq!(from_text.content_type.as_deref(), Some("text/plain"));
    assert_eq!(from_text.data.len(), 1);
    assert_eq!(
        from_text.data[0].data_as_text.as_deref(),
        Some("sample plain text")
    );
    assert!(from_text.data[0].data_as_base64.is_none());

    let val_text = serde_json::to_value(&from_text).expect("serializes from_text");
    assert_eq!(val_text["type"], "text/plain");
    assert_eq!(val_text["data"][0]["data:asText"], "sample plain text");

    let from_b64 = UploadBlob::from_base64("c2FtcGxl", "application/octet-stream");
    assert_eq!(
        from_b64.content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(from_b64.data.len(), 1);
    assert_eq!(from_b64.data[0].data_as_base64.as_deref(), Some("c2FtcGxl"));
    assert!(from_b64.data[0].data_as_text.is_none());

    let val_b64 = serde_json::to_value(&from_b64).expect("serializes from_base64");
    assert_eq!(val_b64["type"], "application/octet-stream");
    assert_eq!(val_b64["data"][0]["data:asBase64"], "c2FtcGxl");
}

#[test]
fn upload_blob_with_data_source_and_extra() {
    let mut extra = BTreeMap::new();
    extra.insert("clientTrackingTag".to_string(), json!("tag-99"));

    let blob = UploadBlob::new()
        .with_data_source(DataSource::as_text("line 1\n"))
        .with_data_source(DataSource::as_text("line 2\n"))
        .with_content_type("text/plain")
        .with_extra(extra);

    assert_eq!(blob.content_type.as_deref(), Some("text/plain"));
    assert_eq!(blob.data.len(), 2);
    assert_eq!(blob.data[0].data_as_text.as_deref(), Some("line 1\n"));
    assert_eq!(blob.data[1].data_as_text.as_deref(), Some("line 2\n"));
    assert_eq!(blob.extra["clientTrackingTag"], json!("tag-99"));

    let val = serde_json::to_value(&blob).expect("serializes upload blob");
    assert_eq!(val["clientTrackingTag"], "tag-99");
    assert_eq!(val["data"][0]["data:asText"], "line 1\n");
    assert_eq!(val["data"][1]["data:asText"], "line 2\n");
}

#[test]
fn blob_upload_response_with_extra() {
    let mut extra = BTreeMap::new();
    extra.insert("quotaRemaining".to_string(), json!(987654321));

    let resp = BlobUploadResponse::new("acc-123").with_extra(extra);
    assert_eq!(resp.account_id, Id::from("acc-123"));
    assert_eq!(resp.extra["quotaRemaining"], json!(987654321));

    let val = serde_json::to_value(&resp).expect("serializes blob upload response");
    assert_eq!(val["accountId"], "acc-123");
    assert_eq!(val["quotaRemaining"], 987654321);

    let round: BlobUploadResponse =
        serde_json::from_value(val).expect("deserializes blob upload response");
    assert_eq!(round.account_id, Id::from("acc-123"));
    assert_eq!(round.extra["quotaRemaining"], json!(987654321));
}

#[test]
fn blob_get_request_with_ids_ref() {
    let res_ref = ResultReference::new("call-upload", "Blob/upload", "/created/*/id");
    let req = BlobGetRequest::new("acc-456", Vec::<Id>::new()).with_ids_ref(res_ref.clone());

    assert_eq!(req.account_id, Id::from("acc-456"));
    assert_eq!(req.ids_ref.as_ref(), Some(&res_ref));
    assert!(req.ids.is_empty());

    let val = serde_json::to_value(&req).expect("serializes blob get request");
    assert_eq!(val["accountId"], "acc-456");
    assert_eq!(val["#ids"]["resultOf"], "call-upload");
    assert_eq!(val["#ids"]["name"], "Blob/upload");
    assert_eq!(val["#ids"]["path"], "/created/*/id");

    let round: BlobGetRequest =
        serde_json::from_value(val).expect("deserializes blob get request with ids_ref");
    assert_eq!(round.account_id, Id::from("acc-456"));
    assert_eq!(round.ids_ref.as_ref(), Some(&res_ref));
}

#[test]
fn data_source_and_upload_result_extra() {
    let mut ds_extra = BTreeMap::new();
    ds_extra.insert("checksum".to_string(), json!("crc32:12345678"));
    let ds = DataSource::as_text("sample").with_extra(ds_extra);
    assert_eq!(ds.extra["checksum"], json!("crc32:12345678"));

    let val_ds = serde_json::to_value(&ds).expect("serializes data source");
    assert_eq!(val_ds["checksum"], "crc32:12345678");

    let mut res_extra = BTreeMap::new();
    res_extra.insert("retentionDays".to_string(), json!(30));
    let res = UploadBlobResult::new("b-result", 4096).with_extra(res_extra);
    assert_eq!(res.id, Id::from("b-result"));
    assert_eq!(res.size, 4096);
    assert_eq!(res.extra["retentionDays"], json!(30));

    let val_res = serde_json::to_value(&res).expect("serializes upload blob result");
    assert_eq!(val_res["retentionDays"], 30);
}

#[test]
fn blob_info_flags_and_properties() {
    let mut info_extra = BTreeMap::new();
    info_extra.insert("customHeader".to_string(), json!("x-val"));

    let info = BlobInfo::new("blob-info-1", 1234)
        .with_is_encoding_problem(true)
        .with_is_truncated(false)
        .with_data("inline text representation")
        .with_content_type("text/plain; charset=utf-8")
        .with_extra(info_extra);

    assert_eq!(info.id, Id::from("blob-info-1"));
    assert_eq!(info.size, Some(1234));
    assert_eq!(info.is_encoding_problem, Some(true));
    assert_eq!(info.is_truncated, Some(false));
    assert_eq!(info.data.as_deref(), Some("inline text representation"));
    assert_eq!(
        info.content_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(info.extra["customHeader"], json!("x-val"));

    let val = serde_json::to_value(&info).expect("serializes blob info");
    assert_eq!(val["isEncodingProblem"], true);
    assert_eq!(val["isTruncated"], false);
    assert_eq!(val["data"], "inline text representation");
    assert_eq!(val["type"], "text/plain; charset=utf-8");
    assert_eq!(val["customHeader"], "x-val");
}

#[test]
fn blob_lookup_match_and_response_builders() {
    let mut matched = BTreeMap::new();
    matched.insert(
        "Email".to_string(),
        vec![Id::from("email-1"), Id::from("email-2")],
    );
    let mut match_extra = BTreeMap::new();
    match_extra.insert("lookupScore".to_string(), json!(0.95));

    let m = BlobLookupMatch::new("blob-match-1")
        .with_matched_ids(matched)
        .with_extra(match_extra);

    assert_eq!(m.id, Id::from("blob-match-1"));
    assert_eq!(
        m.matched_ids.get("Email").map(Vec::as_slice),
        Some(&[Id::from("email-1"), Id::from("email-2")][..])
    );
    assert_eq!(m.extra["lookupScore"], json!(0.95));

    let val_m = serde_json::to_value(&m).expect("serializes blob lookup match");
    assert_eq!(val_m["lookupScore"], 0.95);
    assert_eq!(val_m["matchedIds"]["Email"], json!(["email-1", "email-2"]));

    let resp = BlobLookupResponse::new("acc-lookup", [m]).with_not_found(["blob-missing"]);
    assert_eq!(resp.account_id, Id::from("acc-lookup"));
    assert_eq!(resp.list.len(), 1);
    assert_eq!(resp.not_found, vec![Id::from("blob-missing")]);

    let val_resp = serde_json::to_value(&resp).expect("serializes blob lookup response");
    assert_eq!(val_resp["accountId"], "acc-lookup");
    assert_eq!(val_resp["notFound"], json!(["blob-missing"]));
}
