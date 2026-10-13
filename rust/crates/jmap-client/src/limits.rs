// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! How many octets of response body this client will hold in memory.
//!
//! JMAP bounds what a client *sends* and nothing else: RFC 8620 §2 puts
//! `maxSizeRequest` on a request to `apiUrl` and `maxSizeUpload` on an upload,
//! both checked before sending (see [`Client::api_call`] and
//! [`Client::upload_blob`]). Because the session document gives no limit for
//! response bodies, a client that buffers responses whole must define ceilings.
//!
//! These ceilings bound single buffered response bodies to avoid unbounded
//! allocations while remaining comfortably above expected server payloads.
//!
//! [`Client::api_call`]: crate::Client::api_call
//! [`Client::upload_blob`]: crate::Client::upload_blob

/// The most octets of JSON this client will take as one answer: the session
/// document, every response from `apiUrl`, and upload response descriptors.
///
/// Bounded by the account's `maxSizeRequest` and `maxObjectsInGet`. Message
/// bodies are handled as blobs with their own ceiling below.
pub const MAX_API_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// The most octets an RFC 8414 authorization-server metadata document may be.
///
/// Kept well below [`MAX_API_RESPONSE_BYTES`] because discovery occurs prior to
/// authentication (see [`crate::oauth`]). Real metadata documents are typically
/// a few kilobytes.
pub const MAX_OAUTH_METADATA_BYTES: u64 = 1024 * 1024;

/// The most octets an RFC 7591 dynamic client registration response may be.
///
/// Same reasoning and limit as [`MAX_OAUTH_METADATA_BYTES`].
pub const MAX_OAUTH_REGISTRATION_BYTES: u64 = 1024 * 1024;

/// The most octets an RFC 6749 §5 token endpoint response (success or error)
/// may be.
///
/// Same reasoning and limit as [`MAX_OAUTH_METADATA_BYTES`].
pub const MAX_OAUTH_TOKEN_BYTES: u64 = 1024 * 1024;

/// The most octets one blob download may be when the caller has no better
/// number.
///
/// When available, callers should use the message's stated `size` from
/// `Email/get` (RFC 8621 §4.1.1). This constant serves as a fallback.
pub const MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024;
