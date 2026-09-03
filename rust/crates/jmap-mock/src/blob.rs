// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Blob/get`, `Blob/upload`, and `Blob/lookup` (RFC 9404); plus `Blob/copy`
//! (RFC 8620 §5.7).

use std::collections::BTreeMap;

use base64::Engine;
use jmap_proto::Id;
use jmap_proto::blob::{
    BlobGetRequest, BlobGetResponse, BlobInfo, BlobLookupMatch, BlobLookupRequest,
    BlobLookupResponse, BlobUploadRequest, BlobUploadResponse, DataSource, UploadBlobResult,
    blob_set_error,
};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{BlobCopyRequest, BlobCopyResponse};
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::state::AccountState;

/// `Blob/get`'s default `properties` (RFC 9404 §2.1) when the request names
/// none.
const DEFAULT_PROPERTIES: &[&str] = &["data:asBase64", "size"];

fn slice_bytes(data: &[u8], offset: Option<u64>, length: Option<u64>) -> &[u8] {
    let start = (offset.unwrap_or(0) as usize).min(data.len());
    let end = match length {
        Some(length) => start.saturating_add(length as usize).min(data.len()),
        None => data.len(),
    };
    &data[start..end]
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `Blob/get` (RFC 9404 §2.1). `offset`/`length` slice the underlying octets
/// before any property is derived from them, so a size or digest requested
/// alongside a range reports the range's, not the whole blob's.
pub fn blob_get(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: BlobGetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let properties: Vec<String> = request
        .properties
        .clone()
        .unwrap_or_else(|| DEFAULT_PROPERTIES.iter().map(|p| p.to_string()).collect());

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    for id in &request.ids {
        let Some(blob) = account.blobs.get(id) else {
            not_found.push(id.clone());
            continue;
        };
        let slice = slice_bytes(&blob.data, request.offset, request.length);

        let mut info = BlobInfo::from_id(id.clone());
        for property in &properties {
            info = match property.as_str() {
                "size" => info.with_size(slice.len() as u64),
                "type" => info.with_content_type(blob.content_type.clone()),
                "data:asText" => match std::str::from_utf8(slice) {
                    Ok(text) => info.with_data_as_text(text),
                    Err(_) => info,
                },
                "data:asBase64" => info
                    .with_data_as_base64(base64::engine::general_purpose::STANDARD.encode(slice)),
                "digest:sha-256" => info.with_digest("sha-256", to_hex(&Sha256::digest(slice))),
                "digest:sha-512" => info.with_digest("sha-512", to_hex(&Sha512::digest(slice))),
                _ => info,
            };
        }
        list.push(info);
    }

    to_result(&BlobGetResponse::new(request.account_id, list).with_not_found(not_found))
}

/// Resolve one `Blob/upload` creation's data sources (RFC 9404 §2.2) to the
/// octet stream they concatenate into, or the `SetError` that stops it.
fn resolve_data(account: &AccountState, sources: &[DataSource]) -> Result<Vec<u8>, SetError> {
    let mut bytes = Vec::new();
    for source in sources {
        if let Some(text) = &source.data_as_text {
            bytes.extend_from_slice(text.as_bytes());
        } else if let Some(base64_data) = &source.data_as_base64 {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(base64_data)
                .map_err(|decode_error| {
                    SetError::new(error::set::INVALID_PROPERTIES).with_description(format!(
                        "data:asBase64 is not valid base64: {decode_error}"
                    ))
                })?;
            bytes.extend_from_slice(&decoded);
        } else if let Some(blob_id) = &source.blob_id {
            let existing = account
                .blobs
                .get(blob_id)
                .ok_or_else(|| SetError::new(blob_set_error::BLOB_NOT_FOUND))?;
            bytes.extend_from_slice(slice_bytes(&existing.data, source.offset, source.length));
        } else {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("a data source needs data:asText, data:asBase64, or blobId"));
        }
    }
    Ok(bytes)
}

/// `Blob/upload` (RFC 9404 §2.2). Every `create` entry is independent: one
/// entry's failure does not stop the others from being stored.
pub fn blob_upload(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: BlobUploadRequest = parse_arguments(arguments)?;
    let BlobUploadRequest { account_id, create } = request;
    account_mut(state, &account_id)?;

    let mut created = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    for (creation_id, upload) in create {
        let account = account_mut(state, &account_id)?;
        match resolve_data(account, &upload.data) {
            Ok(bytes) => {
                let content_type = upload
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_owned());
                let size = bytes.len() as u64;
                let id = account_mut(state, &account_id)?.add_blob(content_type.clone(), bytes);
                created.insert(
                    creation_id,
                    UploadBlobResult::new(id, size).with_content_type(content_type),
                );
            }
            Err(set_error) => {
                not_created.insert(creation_id, set_error);
            }
        }
    }

    to_result(
        &BlobUploadResponse::new(account_id)
            .with_created(created)
            .with_not_created(not_created),
    )
}

/// `Blob/lookup` (RFC 9404 §2.3): reports whether each id is a known blob.
/// This mock keeps a flat blob store with no reverse index of which other
/// objects reference a blob id, so every `matchedIds` list stays empty
/// rather than guessing.
pub fn blob_lookup(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: BlobLookupRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    for id in &request.ids {
        if !account.blobs.contains_key(id) {
            not_found.push(id.clone());
            continue;
        }
        let mut item = BlobLookupMatch::new(id.clone());
        for type_name in &request.type_names {
            item = item.with_type_matched_ids(type_name.clone(), Vec::<Id>::new());
        }
        list.push(item);
    }

    to_result(&BlobLookupResponse::new(request.account_id, list).with_not_found(not_found))
}

/// `Blob/copy` (RFC 8620 §5.7): copies blobs from `fromAccountId` into
/// `accountId`, each getting a fresh id in the target account. An unknown
/// source account fails the whole call with `fromAccountNotFound`, since
/// there is nowhere to read from; a missing individual blob id only fails
/// that one entry, reported in `notCopied`.
pub fn blob_copy(
    state: &mut crate::state::ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: BlobCopyRequest = parse_arguments(arguments)?;
    let BlobCopyRequest {
        from_account_id,
        account_id,
        blob_ids,
    } = request;

    let source = state
        .account(&from_account_id)
        .ok_or_else(|| MethodError::new(error::method::FROM_ACCOUNT_NOT_FOUND))?;
    let sources: Vec<Option<(String, Vec<u8>)>> = blob_ids
        .iter()
        .map(|id| {
            source
                .blobs
                .get(id)
                .map(|blob| (blob.content_type.clone(), blob.data.clone()))
        })
        .collect();
    account_mut(state, &account_id)?;

    let mut copied = BTreeMap::new();
    let mut not_copied = BTreeMap::new();
    for (id, blob) in blob_ids.into_iter().zip(sources) {
        match blob {
            Some((content_type, data)) => {
                let new_id = account_mut(state, &account_id)?.add_blob(content_type, data);
                copied.insert(id, new_id);
            }
            None => {
                not_copied.insert(id, SetError::new(blob_set_error::BLOB_NOT_FOUND));
            }
        }
    }

    to_result(
        &BlobCopyResponse::new(from_account_id, account_id)
            .with_copied(copied)
            .with_not_copied(not_copied),
    )
}
