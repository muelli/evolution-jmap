// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Blob Management (RFC 9404) against the real mock server:
//! `Blob/upload`, `Blob/get`, and `Blob/lookup`. `jmap-client/tests/
//! blob_methods.rs` pins the wire shape against a hand-rolled stub; this
//! file exercises `jmap-mockd`'s own implementation instead, per the
//! project's standing rule that every client method is TDD'd against the
//! mock.

use jmap_client::{Client, Credentials};
use jmap_proto::blob::{BlobGetRequest, BlobLookupRequest, BlobUploadRequest, DataSource};
use jmap_proto::session::CAPABILITY_BLOB;

/// A server advertises `urn:ietf:params:jmap:blob` both at session level and
/// on the account, the same way every other capability does, and the typed
/// capability parses with the type names `Blob/lookup` accepts.
#[test]
fn blob_capability_is_advertised_and_resolves_to_the_account() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let capability = client.session().blob_capability();
    assert!(capability.is_some());
    assert!(
        capability
            .unwrap()
            .supported_type_names
            .unwrap()
            .contains(&"Email".to_owned())
    );
    assert_eq!(
        client.session().resolve_primary_account(CAPABILITY_BLOB),
        Some(&account_id)
    );
}

/// `Blob/upload` stores the concatenated data sources and answers a fresh
/// id; the very same id then round-trips through `Blob/get`.
#[test]
fn upload_then_get_round_trips_the_content() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let upload_request = BlobUploadRequest::new(account_id.clone()).create_blob(
        "b0",
        jmap_proto::blob::UploadBlob::from_text("hello world", "text/plain"),
    );
    let upload_response = client.blob_upload(&upload_request).expect("blob_upload");
    let created = upload_response.created.expect("at least one blob created");
    let result = created.get("b0").expect("b0 was created");
    assert_eq!(result.size, 11);
    assert_eq!(result.content_type.as_deref(), Some("text/plain"));

    let get_request = BlobGetRequest::new(account_id.clone(), [result.id.clone()])
        .with_properties(["data:asText", "size", "type", "digest:sha-256"]);
    let get_response = client.blob_get(&get_request).expect("blob_get");
    assert!(get_response.not_found.is_empty());
    let info = &get_response.list[0];
    assert_eq!(info.data_as_text.as_deref(), Some("hello world"));
    assert_eq!(info.size, Some(11));
    assert_eq!(info.content_type.as_deref(), Some("text/plain"));
    assert_eq!(
        info.digest("sha-256"),
        Some("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
    );
}

/// `Blob/get`'s `offset`/`length` slice the octets before any other property
/// is derived from the slice, so a truncated range reports its own size.
#[test]
fn blob_get_offset_and_length_slice_before_deriving_properties() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let upload_request = BlobUploadRequest::new(account_id.clone()).create_blob(
        "b0",
        jmap_proto::blob::UploadBlob::from_text("hello world", "text/plain"),
    );
    let created = client
        .blob_upload(&upload_request)
        .expect("blob_upload")
        .created
        .expect("blob created");
    let blob_id = created.get("b0").expect("b0 was created").id.clone();

    let get_request = BlobGetRequest::new(account_id, [blob_id])
        .with_properties(["data:asText", "size"])
        .with_offset(6)
        .with_length(5);
    let response = client.blob_get(&get_request).expect("blob_get");
    let info = &response.list[0];
    assert_eq!(info.data_as_text.as_deref(), Some("world"));
    assert_eq!(info.size, Some(5));
}

/// `Blob/upload` with a `blobId` data source appends a slice of an existing
/// blob's data rather than uploading raw octets (RFC 9404 §2.2).
#[test]
fn blob_upload_from_an_existing_blob_id_slices_it() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let first = client
        .blob_upload(&BlobUploadRequest::new(account_id.clone()).create_blob(
            "b0",
            jmap_proto::blob::UploadBlob::from_text("hello world", "text/plain"),
        ))
        .expect("first blob_upload")
        .created
        .expect("blob created");
    let first_id = first.get("b0").expect("b0 was created").id.clone();

    let second_request = BlobUploadRequest::new(account_id.clone()).create_blob(
        "b1",
        jmap_proto::blob::UploadBlob::new()
            .with_data_source(DataSource::from_blob_id(first_id).with_length(5))
            .with_content_type("text/plain"),
    );
    let second = client
        .blob_upload(&second_request)
        .expect("second blob_upload")
        .created
        .expect("blob created");
    let second_id = second.get("b1").expect("b1 was created").id.clone();

    let get_response = client
        .blob_get(&BlobGetRequest::new(account_id, [second_id]).with_properties(["data:asText"]))
        .expect("blob_get");
    assert_eq!(get_response.list[0].data_as_text.as_deref(), Some("hello"));
}

/// A missing `blobId` in a `Blob/upload` data source fails that one
/// creation with `blobNotFound`, without affecting sibling creations in the
/// same call.
#[test]
fn blob_upload_from_a_missing_blob_id_is_not_created() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = BlobUploadRequest::new(account_id)
        .create_blob(
            "good",
            jmap_proto::blob::UploadBlob::from_text("hi", "text/plain"),
        )
        .create_blob(
            "bad",
            jmap_proto::blob::UploadBlob::new()
                .with_data_source(DataSource::from_blob_id("nonexistent"))
                .with_content_type("text/plain"),
        );
    let response = client.blob_upload(&request).expect("blob_upload");
    assert!(response.created.as_ref().unwrap().contains_key("good"));
    let not_created = response.not_created.expect("bad creation reported");
    assert_eq!(not_created["bad"].error_type, "blobNotFound");
}

/// `Blob/lookup` tells an existing blob id apart from a missing one; the
/// mock keeps no reverse index from a blob to what references it, so a
/// freshly uploaded blob matches nothing yet for any requested type.
#[test]
fn blob_lookup_distinguishes_known_from_missing_ids() {
    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .blob_upload(&BlobUploadRequest::new(account_id.clone()).create_blob(
            "b0",
            jmap_proto::blob::UploadBlob::from_text("hi", "text/plain"),
        ))
        .expect("blob_upload")
        .created
        .expect("blob created");
    let blob_id = created.get("b0").expect("b0 was created").id.clone();

    let request = BlobLookupRequest::new(
        account_id,
        ["Email"],
        [blob_id.clone(), jmap_proto::Id::from("missing")],
    );
    let response = client.blob_lookup(&request).expect("blob_lookup");
    assert_eq!(response.not_found, vec![jmap_proto::Id::from("missing")]);
    assert_eq!(response.list.len(), 1);
    assert_eq!(response.list[0].id, blob_id);
    assert_eq!(response.list[0].matched_ids.get("Email"), Some(&vec![]));
}
