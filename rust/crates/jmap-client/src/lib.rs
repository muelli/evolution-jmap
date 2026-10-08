// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![forbid(unsafe_code)]

//! Blocking JMAP client.
//!
//! Talks to a JMAP server over HTTP: session discovery
//! (`/.well-known/jmap`), method batching against the API endpoint, and blob
//! upload/download. HTTP is abstracted behind the [`Transport`] trait so the
//! default `ureq` implementation can later be replaced by a libsoup-backed
//! one inside Evolution Data Server processes; [`CancelFlag`] is the seam
//! that will map to `GCancellable`.
//!
//! [`Transport`]: transport::Transport
//! [`CancelFlag`]: transport::CancelFlag
//!
//! # Minimal Working Example
//!
//! ```rust
//! use std::time::Duration;
//! use jmap_client::{Client, Credentials};
//! use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};
//!
//! struct MockTransport;
//!
//! impl Transport for MockTransport {
//!     fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
//!         Ok(HttpResponse {
//!             status: 200,
//!             content_type: Some("application/json".to_string()),
//!             body: br#"{
//!                 "capabilities": {
//!                     "urn:ietf:params:jmap:core": {
//!                         "maxSizeUpload": 50000000,
//!                         "maxConcurrentUpload": 4,
//!                         "maxSizeRequest": 10000000,
//!                         "maxConcurrentRequests": 4,
//!                         "maxCallsInRequest": 16,
//!                         "maxObjectsInGet": 500,
//!                         "maxObjectsInSet": 500,
//!                         "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap", "i;octet"]
//!                     }
//!                 },
//!                 "accounts": {
//!                     "acc1": {
//!                         "name": "user@example.com",
//!                         "isPersonal": true,
//!                         "isReadOnly": false,
//!                         "accountCapabilities": {
//!                             "urn:ietf:params:jmap:core": {}
//!                         }
//!                     }
//!                 },
//!                 "primaryAccounts": {
//!                     "urn:ietf:params:jmap:core": "acc1"
//!                 },
//!                 "username": "user@example.com",
//!                 "apiUrl": "https://example.com/api",
//!                 "downloadUrl": "https://example.com/download/{blobId}",
//!                 "uploadUrl": "https://example.com/upload",
//!                 "eventSourceUrl": "https://example.com/events",
//!                 "state": "init-state"
//!             }"#.to_vec(),
//!             final_url: req.url.to_string(),
//!         })
//!     }
//! }
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = Client::builder()
//!         .transport(MockTransport)
//!         .timeout(Duration::from_secs(10))
//!         .connect("https://example.com", Credentials::bearer("secret-token"))?;
//!
//!     assert!(!client.is_anonymous());
//!     assert_eq!(client.session().username, "user@example.com");
//!     assert_eq!(
//!         client.primary_account("urn:ietf:params:jmap:core")?.as_str(),
//!         "acc1"
//!     );
//!     Ok(())
//! }
//! ```

mod blob;
mod calendars;
mod changes;
mod client;
mod contacts;
mod error;
pub mod eventsource;
pub mod limits;
mod mail;
pub mod oauth;
mod principals;
mod quota;
pub mod resolver;
mod sieve;
mod snooze;
pub mod transport;
mod url;

pub use changes::ChangeSet;
pub use client::{Client, ClientBuilder, Credentials, rebase_urls_from_env};
pub use error::Error;
pub use eventsource::{
    EventSourceItem, EventSourceSubscription, EventSourceTimeouts, SharedHeaders, expand_url,
};
pub use jmap_proto as proto;
pub use jmap_proto;
pub use jmap_proto::{Id, State};
pub use transport::CancelFlag;
