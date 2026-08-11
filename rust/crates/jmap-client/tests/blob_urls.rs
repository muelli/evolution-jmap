// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Blob URLs built from the session's templates, with server-chosen values in
//! them.
//!
//! RFC 8620 §6.2 requires the client to URI-encode every value it substitutes
//! into `downloadUrl`/`uploadUrl`. None of the three is this client's to
//! choose: the `accountId` and the `blobId` come out of the session document
//! and out of `Email/get`, and `jmap_proto::Id` puts no grammar on either; the
//! `name` is whatever the caller labels the download with. A value carrying
//! `#`, `?`, `/` or a space that is pasted in verbatim does not name the blob
//! it came from — it names a different URL.

use jmap_client::{Client, Credentials, limits};
use jmap_mock::{AccountState, Blob, MockServer};
use jmap_proto::Id;

/// Every character that changes what a URL means, in one id: the fragment
/// mark, the query mark, a path separator, a space, and the escape itself.
const HOSTILE: &str = "b#1?2/3 4%5";

#[test]
fn a_blob_id_with_url_syntax_downloads_the_blob_it_names() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let blob_id = Id::new(HOSTILE);
    let data = b"the blob the id names".to_vec();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        account.blobs.insert(
            blob_id.clone(),
            Blob {
                content_type: "application/octet-stream".to_owned(),
                data: data.clone(),
            },
        );
    }

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let downloaded = client
        .download_blob(
            &account_id,
            &blob_id,
            "attachment.bin",
            limits::MAX_BLOB_BYTES,
        )
        .unwrap();

    assert_eq!(downloaded, data);
}

#[test]
fn a_download_name_with_url_syntax_stays_one_path_segment() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let data = b"named oddly".to_vec();
    let uploaded = client
        .upload_blob(&account_id, "application/octet-stream", data.clone())
        .unwrap();
    // The name is decoration in the URL — a filename hint for the browser —
    // but an unencoded one still reshapes the path it sits in.
    let downloaded = client
        .download_blob(
            &account_id,
            &uploaded.blob_id,
            "../../etc/passwd?x=1",
            limits::MAX_BLOB_BYTES,
        )
        .unwrap();

    assert_eq!(downloaded, data);
}

#[test]
fn an_account_id_with_url_syntax_uploads_and_downloads() {
    let server = MockServer::builder().start();
    let account_id = Id::new(HOSTILE);
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .accounts
            .insert(account_id.clone(), AccountState::new("Hostile"));
    }

    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let data = b"in an account with an awkward id".to_vec();
    let uploaded = client
        .upload_blob(&account_id, "application/octet-stream", data.clone())
        .unwrap();
    let downloaded = client
        .download_blob(
            &account_id,
            &uploaded.blob_id,
            "blob.bin",
            limits::MAX_BLOB_BYTES,
        )
        .unwrap();

    assert_eq!(downloaded, data);
}
