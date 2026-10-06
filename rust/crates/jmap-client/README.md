<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# evolution-jmap-client

A blocking, embeddable JMAP (JSON Meta Application Protocol) client in Rust.

This crate provides a synchronous client implementation for interacting with JMAP
servers: session discovery (`/.well-known/jmap`), method batching, blob upload and
download, change tracking, and server-sent event streams.

## Features and Positioning

Stalwart's `jmap-client` crate is an established async-first client designed around
Tokio. `evolution-jmap-client` is designed for distinct architectural needs:

- **Blocking and synchronous**: Minimal dependency footprint without forcing an
  async runtime (such as Tokio) onto embedders.
- **Pluggable transport**: Network I/O is abstracted behind the [`Transport`] trait.
  The default synchronous implementation uses `ureq`, but embedders can easily
  substitute custom networking (such as libsoup inside Evolution Data Server).
- **First-class cancellation**: Supports thread-local and client-wide [`CancelFlag`]
  tokens that map cleanly onto GObject `GCancellable`.
- **Built-in OAuth 2.0**: Full RFC 8414 authorization server metadata discovery,
  RFC 7591 dynamic client registration, and RFC 7636 PKCE authorization code
  exchange.
- **Service autodiscovery**: Implements RFC 8620 §2.2 DNS SRV lookup via an
  abstract [`Resolver`] seam without imposing external DNS library dependencies.
- **Broad protocol coverage**: Typed APIs for Core (RFC 8620), Mail (RFC 8621),
  Contacts (RFC 9610), Calendars (draft-ietf-jmap-calendars), Sharing and Principals
  (RFC 9670), Quotas (RFC 9425), Sieve (RFC 9661), and Web Push (RFC 9749).
- **Real-world tolerance**: Hardened to tolerate observed server behaviors and
  deviations ("do whatever Stalwart does") such as unauthenticated session responses,
  account ID principal mappings, and omitted response state tokens.

## Minimal Working Example

```rust
use std::time::Duration;
use jmap_client::{Client, Credentials};
use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};

struct MockTransport;

impl Transport for MockTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            body: br#"{
                "capabilities": {
                    "urn:ietf:params:jmap:core": {
                        "maxSizeUpload": 50000000,
                        "maxConcurrentUpload": 4,
                        "maxSizeRequest": 10000000,
                        "maxConcurrentRequests": 4,
                        "maxCallsInRequest": 16,
                        "maxObjectsInGet": 500,
                        "maxObjectsInSet": 500,
                        "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap", "i;octet"]
                    }
                },
                "accounts": {
                    "acc1": {
                        "name": "user@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {
                            "urn:ietf:params:jmap:core": {}
                        }
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:core": "acc1"
                },
                "username": "user@example.com",
                "apiUrl": "https://example.com/api",
                "downloadUrl": "https://example.com/download/{blobId}",
                "uploadUrl": "https://example.com/upload",
                "eventSourceUrl": "https://example.com/events",
                "state": "init-state"
            }"#.to_vec(),
            final_url: req.url.to_string(),
        })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .transport(MockTransport)
        .timeout(Duration::from_secs(10))
        .connect("https://example.com", Credentials::bearer("secret-token"))?;

    assert!(!client.is_anonymous());
    assert_eq!(client.session().username, "user@example.com");
    assert_eq!(
        client.primary_account("urn:ietf:params:jmap:core")?.as_str(),
        "acc1"
    );
    Ok(())
}
```

## Feature flags

| Feature | Default | Description |
|---|---|---|
| `transport-ureq` | Yes | Enables the default blocking HTTP transport backed by `ureq` |
| `live-server` | No | Enables opt-in integration tests against a live JMAP deployment |

## Minimum Supported Rust Version (MSRV)

This crate requires **Rust 1.97** or newer and uses the Rust 2024 edition.

## License

This project is licensed under the **GNU General Public License v3.0 or later**
(`GPL-3.0-or-later`). See the repository root for full license details.

## Scope statement

The scope of this crate follows the project's standing rule: conform strictly
to published standards while being tolerant of real-world server implementations
("do whatever Stalwart does").

- **Robust client operations**: Manages HTTP request envelopes, method call ID
  tracking, result reference evaluation, and transparent URL rebasing.
- **Tolerant response handling**: Handles non-fatal quirks in server responses,
  such as omitted properties on create/destroy or alternative authentication statuses.

### What is not implemented

- **Asynchronous I/O**: This crate is deliberately synchronous and blocking. For an
  asynchronous, Tokio-based JMAP client, see Stalwart's `jmap-client`.
- **Server dispatch and storage**: No server-side storage or request execution engine
  is provided. For testing against an in-memory mock server, see `evolution-jmap-mock`.
- **Format conversion**: Semantic translation between legacy calendar/contact formats
  (iCalendar RFC 5545, vCard RFC 6350) and JSCalendar/JSContact is implemented in
  `evolution-jmap-ical` and `evolution-jmap-vcard`.

## Documentation gaps (TODO)

Module-level and architectural documentation covers public entry points and traits.
Approximately 52 item-level doc comments across error variants, internal fields, and
helper methods are tracked for continuous documentation refinement.
