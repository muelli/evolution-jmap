// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for client API ergonomics, re-exports, and standard trait implementations.

use std::error::Error as StdError;
use std::time::Duration;

use jmap_client::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use jmap_client::{
    Client, ClientBuilder, Credentials, EventSourceItem, EventSourceSubscription,
    EventSourceTimeouts, Id, SharedHeaders, State, expand_url, proto,
};

struct MockTransport;

impl Transport for MockTransport {
    fn execute(&self, req: HttpRequest<'_>) -> Result<HttpResponse, TransportError> {
        let body = if req.url.ends_with("/.well-known/jmap") {
            br#"{
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
                "uploadUrl": "https://example.com/upload/{accountId}",
                "eventSourceUrl": "https://example.com/events?types={types}&closeafter={closeafter}&ping={ping}",
                "state": "init-state"
            }"#.to_vec()
        } else if req.url.contains("/upload/") {
            br#"{
                "accountId": "acc1",
                "blobId": "blob-1234",
                "type": "text/plain",
                "size": 11
            }"#
            .to_vec()
        } else {
            b"{}".to_vec()
        };

        Ok(HttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            body,
            final_url: req.url.to_string(),
        })
    }
}

#[test]
fn re_exports_proto_and_eventsource_types() {
    let id: Id = Id::from("test-id");
    assert_eq!(id.as_str(), "test-id");

    let state: State = State::from("s-1");
    assert_eq!(state.as_str(), "s-1");

    let expanded = expand_url(
        "https://example.com/events?types={types}",
        &["Mailbox"],
        false,
        0,
    );
    assert!(expanded.contains("types=Mailbox"));

    let headers = SharedHeaders::new(vec![("X-Test".to_string(), "Value".to_string())]);
    let debug_headers = format!("{headers:?}");
    assert!(debug_headers.contains("SharedHeaders"));

    let _: proto::methods::GetResponse<proto::mail::Mailbox>;
}

#[test]
fn transport_error_implements_display_and_error() {
    let err_cancelled = TransportError::Cancelled;
    assert_eq!(format!("{err_cancelled}"), "transport request cancelled");

    let err_failed = TransportError::Failed("broken pipe".to_string());
    assert_eq!(format!("{err_failed}"), "transport error: broken pipe");

    let err_too_large = TransportError::ResponseTooLarge { limit: 1048576 };
    assert_eq!(
        format!("{err_too_large}"),
        "response body exceeded maximum limit of 1048576 bytes"
    );

    let std_err: &dyn StdError = &err_failed;
    assert_eq!(std_err.to_string(), "transport error: broken pipe");
}

#[test]
fn client_builder_implements_debug() {
    let builder = ClientBuilder::default();
    let debug = format!("{builder:?}");
    assert!(debug.contains("ClientBuilder"));
    assert!(debug.contains("timeout"));
}

#[test]
fn client_upload_blob_slice_and_event_source_conveniences() {
    let client = Client::builder()
        .transport(MockTransport)
        .connect("https://example.com", Credentials::bearer("token"))
        .expect("client connect");

    let account_id = client
        .primary_account("urn:ietf:params:jmap:core")
        .expect("account");

    let payload: &[u8] = b"hello world";
    let upload = client
        .upload_blob_slice(&account_id, "text/plain", payload)
        .expect("upload blob slice");
    assert_eq!(upload.blob_id.as_str(), "blob-1234");

    let mut sub: EventSourceSubscription = client
        .event_source(&["Mailbox"])
        .expect("event source subscription");
    let debug_sub = format!("{sub:?}");
    assert!(debug_sub.contains("EventSourceSubscription"));
    sub.stop();

    let timeouts = EventSourceTimeouts {
        connect: Duration::from_secs(5),
        read_head: Duration::from_secs(5),
        stream_read: Duration::from_secs(30),
    };
    let mut sub_timeouts = client
        .event_source_with_timeouts(&["Mailbox"], timeouts)
        .expect("event source subscription with timeouts");
    let item_timeout = sub_timeouts.recv_item_timeout(Duration::from_millis(50));
    assert!(item_timeout.is_none());
    sub_timeouts.stop();

    let reconnect_item = EventSourceItem::Reconnected {
        last_event_id: Some("ev-1".to_string()),
        reconnect_count: 1,
    };
    assert_eq!(
        format!("{reconnect_item:?}"),
        "Reconnected { last_event_id: Some(\"ev-1\"), reconnect_count: 1 }"
    );
}
