// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! How many octets of response body this client will hold in memory.
//!
//! JMAP bounds what a client *sends* and nothing else: RFC 8620 §2 puts
//! `maxSizeRequest` on a request to `apiUrl` and `maxSizeUpload` on an upload,
//! both checked before the octets leave (see [`Client::api_call`] and
//! [`Client::upload_blob`]), and the session document says nothing at all about
//! how large an answer may be. There is therefore no number to read off the
//! account for a response, and a client that buffers one whole — which this one
//! does, because every response it makes is parsed as a unit — has to pick one.
//!
//! It picks it *here*, in named constants with a reason, because the previous
//! answer was that `ureq`'s `Body::read_to_vec` applies its own `MAX_BODY_SIZE`
//! of 10 MiB. That failed closed rather than truncating, so nothing was ever
//! corrupted by it; it was still a limit nobody in this repository chose, wrote
//! down, or could change, and at 10 MiB it made a single photo attachment the
//! largest message an account could open.
//!
//! Both numbers are ceilings on *one* buffered body, not a memory budget: a
//! provider synchronising several folders at once holds several. They are set
//! where a machine running Evolution would rather refuse a body than swap for
//! it, and far above anything a working server sends.
//!
//! [`Client::api_call`]: crate::Client::api_call
//! [`Client::upload_blob`]: crate::Client::upload_blob

/// The most octets of JSON this client will take as one answer — the session
/// document, every response from `apiUrl`, and the descriptor an upload
/// answers with.
///
/// Sized against the largest question the client asks: a batched `/get` naming
/// ids. What comes back is bounded by what went out, and what went out is
/// already bounded twice over — by the account's `maxSizeRequest`, which the
/// client checks itself, and by `maxObjectsInGet`. Message *bodies* do not come
/// this way at all; they are blobs, and have their own ceiling below. So this
/// is set far above any batch the client builds rather than tuned to one, and
/// its job is to stop a server that answers a small question with an endless
/// body.
pub const MAX_API_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// The most octets an RFC 8414 authorization-server metadata document may be.
///
/// Its own number, well below [`MAX_API_RESPONSE_BYTES`], because it is the
/// one thing this client reads from a server it has not authenticated to and
/// knows almost nothing about — discovery happens before there are
/// credentials, by design (see [`crate::oauth`]). The document itself is a
/// flat JSON object of a few dozen fields; real ones run to a couple of
/// kilobytes, so a mebibyte is three orders of magnitude of room and still
/// small enough that an endpoint answering with an endless body costs nothing.
pub const MAX_OAUTH_METADATA_BYTES: u64 = 1024 * 1024;

/// The most octets an RFC 7591 dynamic client registration response may be.
///
/// Same reasoning and the same number as [`MAX_OAUTH_METADATA_BYTES`]: a flat
/// JSON object of a client id, an optional secret and a handful of echoed
/// metadata fields, read from a server this client has only just discovered.
pub const MAX_OAUTH_REGISTRATION_BYTES: u64 = 1024 * 1024;

/// The most octets an RFC 6749 §5 token endpoint response (success or error)
/// may be.
///
/// Same reasoning and the same number as [`MAX_OAUTH_METADATA_BYTES`]: a flat
/// JSON object of a handful of token fields, or of an `error`/
/// `error_description` pair, from an endpoint this client has by definition
/// not yet finished authenticating to.
pub const MAX_OAUTH_TOKEN_BYTES: u64 = 1024 * 1024;

/// The most octets one blob download may be when the caller has no better
/// number.
///
/// A caller usually does have one: `Email/get` reports each message's `size`,
/// which RFC 8621 §4.1.1 defines as the octets of the raw data the `blobId`
/// refers to — the download's own length. A ceiling taken from the row is the
/// account's number rather than this one, and is what
/// `jmap_mail_sync::MailSync::message_source` passes. This constant is the
/// fallback for a server that reports no size, and the answer to "how large a
/// message will this provider open at all".
pub const MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024;
