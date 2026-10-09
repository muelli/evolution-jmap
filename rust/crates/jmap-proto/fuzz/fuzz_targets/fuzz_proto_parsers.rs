// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Attempt deserialization of JMAP response envelope and top-level object types.
    // Deserialization failure (Err) is expected and normal for arbitrary input:
    // this fuzzer asserts that parser execution never panics, aborts, or hangs.
    if let Ok(resp) = serde_json::from_slice::<jmap_proto::response::Response>(data) {
        let _ = serde_json::to_vec(&resp);
    }
    if let Ok(session) = serde_json::from_slice::<jmap_proto::session::Session>(data) {
        let _ = serde_json::to_vec(&session);
    }
    if let Ok(mailbox) = serde_json::from_slice::<jmap_proto::mail::Mailbox>(data) {
        let _ = serde_json::to_vec(&mailbox);
    }
    if let Ok(email) = serde_json::from_slice::<jmap_proto::mail::Email>(data) {
        let _ = serde_json::to_vec(&email);
    }
    if let Ok(card) = serde_json::from_slice::<jmap_proto::contacts::ContactCard>(data) {
        let _ = serde_json::to_vec(&card);
    }
    if let Ok(event) = serde_json::from_slice::<jmap_proto::calendars::CalendarEvent>(data) {
        let _ = serde_json::to_vec(&event);
    }
});
