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

/// Maximum number of distinct ids that one sync keeps in memory.
///
/// A server can answer many pages, each under [`MAX_API_RESPONSE_BYTES`].
/// This limit applies to the total over all pages.
pub const MAX_SYNC_IDS: usize = 1_000_000;

/// Maximum number of id bytes that one sync keeps in memory.
///
/// RFC 8620 Section 1.2 limits one id to 255 octets, but the client does not check this.
pub const MAX_SYNC_ID_BYTES: usize = 64 * 1024 * 1024;

/// The ids and id bytes that one sync can still keep.
///
/// Start each sync with a fresh copy, and charge each id when the sync keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdBudget {
    ids: usize,
    bytes: usize,
}

impl IdBudget {
    /// A budget of `ids` ids with `bytes` bytes of id text in total.
    pub const fn new(ids: usize, bytes: usize) -> Self {
        Self { ids, bytes }
    }

    /// The budget for one sync: [`MAX_SYNC_IDS`] and [`MAX_SYNC_ID_BYTES`].
    pub const fn per_sync() -> Self {
        Self::new(MAX_SYNC_IDS, MAX_SYNC_ID_BYTES)
    }

    /// Charge one kept id. Returns `false`, and charges nothing, if the id does not fit.
    #[must_use]
    pub fn charge(&mut self, id: &str) -> bool {
        match (self.ids.checked_sub(1), self.bytes.checked_sub(id.len())) {
            (Some(ids), Some(bytes)) => {
                *self = Self { ids, bytes };
                true
            }
            _ => false,
        }
    }
}

impl Default for IdBudget {
    fn default() -> Self {
        Self::per_sync()
    }
}
