// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live verification for JMAP Blob Management (RFC 9404) against a real JMAP
// server (such as Stalwart): confirms capability advertisement, round-trips a
// blob through Blob/upload and Blob/get (including offset/length slicing and
// dynamic digests), and tests Blob/lookup. Prints PASS/FAIL per check and exits
// non-zero on any failure.
//
// Usage:
//   cargo run -p evolution-jmap-client --example blob-capability-probe -- \
//       <origin> <user> <password>

use jmap_client::{Client, Credentials};
use jmap_proto::Id;
use jmap_proto::blob::{
    BlobGetRequest, BlobLookupRequest, BlobUploadRequest, DataSource, UploadBlob,
};
use jmap_proto::session::CAPABILITY_BLOB;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(origin), Some(user), Some(pass)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: blob-capability-probe <origin> <user> <password>");
        std::process::exit(2);
    };

    let c = Client::connect(&origin, Credentials::basic(user, pass)).expect("connect");
    let account = c
        .primary_account(CAPABILITY_BLOB)
        .or_else(|_| c.primary_account("urn:ietf:params:jmap:mail"))
        .or_else(|_| c.primary_account("urn:ietf:params:jmap:core"))
        .unwrap_or_else(|_| Id::from("c"));

    let mut fail = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!(
            "{} {name}: {detail}",
            if ok {
                "PASS"
            } else {
                fail += 1;
                "FAIL"
            }
        );
    };

    // 1. Capability check on accountCapabilities
    let account_obj = c.session().accounts.get(&account);
    let blob_cap = account_obj.and_then(|a| a.blob_capability());
    let has_blob_cap = blob_cap.is_some();
    let supported_types = blob_cap
        .as_ref()
        .and_then(|cap| cap.supported_type_names.clone())
        .unwrap_or_default();
    let supported_digests = blob_cap
        .as_ref()
        .and_then(|cap| cap.supported_digest_algorithms.clone())
        .unwrap_or_default();

    check(
        "capability advertisement (RFC 9404 §1.1)",
        has_blob_cap,
        format!(
            "account {}: supported types {:?}, supported digests {:?}",
            account.as_str(),
            supported_types,
            supported_digests
        ),
    );

    // 2. Blob/upload (RFC 9404 §2.2)
    let upload_text = "Hello Stalwart RFC 9404";
    let upload = UploadBlob::new()
        .with_data([DataSource::as_text(upload_text)])
        .with_content_type("text/plain");
    let upload_req = BlobUploadRequest::new(account.clone()).create_blob("k1", upload);

    let upload_res = c.blob_upload(&upload_req);
    let upload_ok = upload_res.as_ref().is_ok_and(|resp| {
        resp.created
            .as_ref()
            .and_then(|m| m.get("k1"))
            .is_some_and(|r| r.size == upload_text.len() as u64)
    });

    let uploaded_blob_id = upload_res
        .as_ref()
        .ok()
        .and_then(|resp| resp.created.as_ref())
        .and_then(|m| m.get("k1"))
        .map(|r| r.id.clone());

    check(
        "Blob/upload (RFC 9404 §2.2)",
        upload_ok,
        format!(
            "upload text ({} bytes) -> {:?}",
            upload_text.len(),
            uploaded_blob_id
        ),
    );

    let Some(blob_id) = uploaded_blob_id else {
        println!("\nCannot proceed with Blob/get or Blob/lookup without uploaded blob id");
        std::process::exit(1);
    };

    // 3. Blob/get full (RFC 9404 §2.1)
    let get_req = BlobGetRequest::new(
        account.clone(),
        [blob_id.clone(), Id::from("nonexistent_blob_id")],
    )
    .with_properties(["id", "data:asText", "size", "digest:sha-256"]);
    let get_res = c.blob_get(&get_req);

    let get_ok = get_res.as_ref().is_ok_and(|resp| {
        let found = resp.list.iter().find(|b| b.id == blob_id);
        let has_text = found.and_then(|b| b.data_as_text.as_deref()) == Some(upload_text);
        let has_size = found.and_then(|b| b.size) == Some(upload_text.len() as u64);
        let missing = resp.not_found.contains(&Id::from("nonexistent_blob_id"));
        has_text && has_size && missing
    });

    let sha256_digest = get_res
        .as_ref()
        .ok()
        .and_then(|resp| resp.list.iter().find(|b| b.id == blob_id))
        .and_then(|b| b.digest("sha-256"))
        .unwrap_or("none");

    check(
        "Blob/get full and notFound (RFC 9404 §2.1)",
        get_ok,
        format!("retrieved text matches, sha-256: {sha256_digest}"),
    );

    // 4. Blob/get with offset and length slice
    let slice_req = BlobGetRequest::new(account.clone(), [blob_id.clone()])
        .with_properties(["id", "data:asText", "size"])
        .with_offset(6)
        .with_length(8);
    let slice_res = c.blob_get(&slice_req);

    let slice_ok = slice_res.as_ref().is_ok_and(|resp| {
        let found = resp.list.iter().find(|b| b.id == blob_id);
        let has_slice = found.and_then(|b| b.data_as_text.as_deref()) == Some("Stalwart");
        let full_size = found.and_then(|b| b.size) == Some(upload_text.len() as u64);
        has_slice && full_size
    });

    check(
        "Blob/get sliced data with full size (RFC 9404 §2.1)",
        slice_ok,
        "offset 6, length 8 returned sliced text with original size".to_string(),
    );

    // 5. Blob/lookup (RFC 9404 §2.3)
    let lookup_types = if supported_types.is_empty() {
        vec!["Email".to_string()]
    } else {
        supported_types
    };
    let lookup_req = BlobLookupRequest::new(account.clone(), lookup_types, [blob_id.clone()]);
    let lookup_res = c.blob_lookup(&lookup_req);

    let lookup_ok = lookup_res
        .as_ref()
        .is_ok_and(|resp| resp.list.iter().any(|m| m.id == blob_id));

    check(
        "Blob/lookup (RFC 9404 §2.3)",
        lookup_ok,
        format!("lookup for blob {} accepted", blob_id.as_str()),
    );

    if fail == 0 {
        println!("\nALL CHECKS PASSED");
    } else {
        println!("\n{fail} CHECK(S) FAILED");
        std::process::exit(1);
    }
}
