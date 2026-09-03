// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Blob Management (RFC 9404): `Blob/get`, `Blob/upload`, and
//! `Blob/lookup`; plus `Blob/copy` (RFC 8620 §5.7).

use jmap_proto::blob::{
    BlobGetRequest, BlobGetResponse, BlobLookupRequest, BlobLookupResponse, BlobUploadRequest,
    BlobUploadResponse,
};
use jmap_proto::methods::{BlobCopyRequest, BlobCopyResponse};
use jmap_proto::session::{CAPABILITY_BLOB, CAPABILITY_CORE};

use crate::client::Client;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_BLOB];

impl Client {
    /// Retrieve blob data and metadata (`Blob/get`, RFC 9404 §2.1).
    pub fn blob_get(&self, request: &BlobGetRequest) -> Result<BlobGetResponse, Error> {
        let arguments = self.single_call(USING, "Blob/get", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Upload one or more blobs (`Blob/upload`, RFC 9404 §2.2).
    pub fn blob_upload(&self, request: &BlobUploadRequest) -> Result<BlobUploadResponse, Error> {
        let arguments = self.single_call(USING, "Blob/upload", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Discover which objects reference a blob (`Blob/lookup`, RFC 9404 §2.3).
    pub fn blob_lookup(&self, request: &BlobLookupRequest) -> Result<BlobLookupResponse, Error> {
        let arguments = self.single_call(USING, "Blob/lookup", request)?;
        Ok(serde_json::from_value(arguments)?)
    }

    /// Copy blobs from one account to another (`Blob/copy`, RFC 8620 §5.7).
    pub fn blob_copy(&self, request: &BlobCopyRequest) -> Result<BlobCopyResponse, Error> {
        let arguments = self.single_call(USING, "Blob/copy", request)?;
        Ok(serde_json::from_value(arguments)?)
    }
}
