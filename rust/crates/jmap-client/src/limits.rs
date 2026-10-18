// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Memory limits for buffered HTTP response bodies.
//!
//! RFC 8620 Section 2 defines `maxSizeRequest` and `maxSizeUpload` for outgoing data.
//! The specification does not define limits for response payloads.
//! This module defines size caps to prevent unbounded memory allocation.

/// Maximum byte limit for JSON API responses.
///
/// Applies to session documents, `apiUrl` responses, and upload descriptors.
/// Message bodies download as blobs under [`MAX_BLOB_BYTES`].
pub const MAX_API_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum byte limit for OAuth authorization server metadata documents (RFC 8414).
///
/// Kept small because discovery occurs before authentication (see [`crate::oauth`]).
pub const MAX_OAUTH_METADATA_BYTES: u64 = 1024 * 1024;

/// Maximum byte limit for OAuth dynamic client registration responses (RFC 7591).
pub const MAX_OAUTH_REGISTRATION_BYTES: u64 = 1024 * 1024;

/// Maximum byte limit for OAuth token endpoint responses (RFC 6749 Section 5).
pub const MAX_OAUTH_TOKEN_BYTES: u64 = 1024 * 1024;

/// Default byte limit for blob downloads when size is unspecified.
///
/// Callers should prefer the stated `size` from `Email/get` when available (RFC 8621 Section 4.1.1).
pub const MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024;
