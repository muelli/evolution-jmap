// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smoke and regression tests for coverage-guided fuzz targets.
//!
//! Asserts that deserializing JMAP envelopes and top-level domain types
//! never panics on arbitrary or malformed byte sequences on stable Rust.

use jmap_proto::calendars::CalendarEvent;
use jmap_proto::contacts::ContactCard;
use jmap_proto::mail::{Email, Mailbox};
use jmap_proto::response::Response;
use jmap_proto::session::Session;

fn parse_all_types(data: &[u8]) {
    if let Ok(resp) = serde_json::from_slice::<Response>(data) {
        let _ = serde_json::to_vec(&resp);
    }
    if let Ok(session) = serde_json::from_slice::<Session>(data) {
        let _ = serde_json::to_vec(&session);
    }
    if let Ok(mailbox) = serde_json::from_slice::<Mailbox>(data) {
        let _ = serde_json::to_vec(&mailbox);
    }
    if let Ok(email) = serde_json::from_slice::<Email>(data) {
        let _ = serde_json::to_vec(&email);
    }
    if let Ok(card) = serde_json::from_slice::<ContactCard>(data) {
        let _ = serde_json::to_vec(&card);
    }
    if let Ok(event) = serde_json::from_slice::<CalendarEvent>(data) {
        let _ = serde_json::to_vec(&event);
    }
}

#[test]
fn malformed_and_truncated_bytes_never_panic() {
    let hostile_inputs: &[&[u8]] = &[
        b"",
        b"{",
        b"[",
        b"}",
        b"]",
        b"null",
        b"true",
        b"12345",
        b"\"unterminated string",
        b"{\"methodResponses\": null}",
        b"{\"methodResponses\": [\"invalid\"]}",
        b"{\"sessionState\": 12345}",
        b"\xFF\xFE\x00\x00",
        b"{\"@type\": \"Card\", \"unknown\": [null, true, {}]}",
        b"{\"@type\": \"Event\", \"start\": \"invalid-date\"}",
        b"{\"\": \"\"}",
    ];

    for input in hostile_inputs {
        parse_all_types(input);
    }
}

#[test]
fn fixture_seeds_deserialize_and_roundtrip_cleanly() {
    let session_bytes = include_bytes!("fixtures/core/session.json");
    let response_bytes = include_bytes!("fixtures/core/response_with_error.json");
    let mailbox_bytes = include_bytes!("fixtures/mail/mailbox.json");
    let email_bytes = include_bytes!("fixtures/mail/email.json");
    let contact_bytes = include_bytes!("fixtures/contacts/contact_card.json");
    let event_bytes = include_bytes!("fixtures/calendars/calendar_event.json");

    parse_all_types(session_bytes);
    parse_all_types(response_bytes);
    parse_all_types(mailbox_bytes);
    parse_all_types(email_bytes);
    parse_all_types(contact_bytes);
    parse_all_types(event_bytes);

    assert!(serde_json::from_slice::<Session>(session_bytes).is_ok());
    assert!(serde_json::from_slice::<Response>(response_bytes).is_ok());
    assert!(serde_json::from_slice::<Mailbox>(mailbox_bytes).is_ok());
    assert!(serde_json::from_slice::<Email>(email_bytes).is_ok());
    assert!(serde_json::from_slice::<ContactCard>(contact_bytes).is_ok());
    assert!(serde_json::from_slice::<CalendarEvent>(event_bytes).is_ok());
}
