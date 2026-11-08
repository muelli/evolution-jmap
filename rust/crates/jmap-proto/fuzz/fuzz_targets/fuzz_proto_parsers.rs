// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Deserialization errors are expected. Verify that deserialization does not panic or abort.
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
    if let Ok(ab) = serde_json::from_slice::<jmap_proto::contacts::AddressBook>(data) {
        let _ = serde_json::to_vec(&ab);
    }
    if let Ok(cal) = serde_json::from_slice::<jmap_proto::calendars::Calendar>(data) {
        let _ = serde_json::to_vec(&cal);
    }
    if let Ok(pi) = serde_json::from_slice::<jmap_proto::calendars::ParticipantIdentity>(data) {
        let _ = serde_json::to_vec(&pi);
    }
    if let Ok(cen) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventNotification>(data)
    {
        let _ = serde_json::to_vec(&cen);
    }
    if let Ok(task) = serde_json::from_slice::<jmap_proto::tasks::Task>(data) {
        let _ = serde_json::to_vec(&task);
    }
    if let Ok(tl) = serde_json::from_slice::<jmap_proto::tasks::TaskList>(data) {
        let _ = serde_json::to_vec(&tl);
    }
    if let Ok(pr) = serde_json::from_slice::<jmap_proto::principals::Principal>(data) {
        let _ = serde_json::to_vec(&pr);
    }
    if let Ok(sn) =
        serde_json::from_slice::<jmap_proto::principals::ShareNotification>(data)
    {
        let _ = serde_json::to_vec(&sn);
    }
    if let Ok(identity) = serde_json::from_slice::<jmap_proto::mail::Identity>(data) {
        let _ = serde_json::to_vec(&identity);
    }
    if let Ok(sub) = serde_json::from_slice::<jmap_proto::mail::EmailSubmission>(data) {
        let _ = serde_json::to_vec(&sub);
    }
    if let Ok(th) = serde_json::from_slice::<jmap_proto::mail::Thread>(data) {
        let _ = serde_json::to_vec(&th);
    }
    if let Ok(snip) = serde_json::from_slice::<jmap_proto::mail::SearchSnippet>(data) {
        let _ = serde_json::to_vec(&snip);
    }
    if let Ok(script) = serde_json::from_slice::<jmap_proto::sieve::SieveScript>(data) {
        let _ = serde_json::to_vec(&script);
    }
    if let Ok(quota) = serde_json::from_slice::<jmap_proto::quota::Quota>(data) {
        let _ = serde_json::to_vec(&quota);
    }
    if let Ok(fn_node) = serde_json::from_slice::<jmap_proto::filenode::FileNode>(data) {
        let _ = serde_json::to_vec(&fn_node);
    }
    if let Ok(blob_up) = serde_json::from_slice::<jmap_proto::blob::BlobUploadResponse>(data) {
        let _ = serde_json::to_vec(&blob_up);
    }
    if let Ok(blob_get) = serde_json::from_slice::<jmap_proto::blob::BlobGetResponse>(data) {
        let _ = serde_json::to_vec(&blob_get);
    }
    if let Ok(ps) = serde_json::from_slice::<jmap_proto::push::PushSubscription>(data) {
        let _ = serde_json::to_vec(&ps);
    }
    if let Ok(sc) = serde_json::from_slice::<jmap_proto::push::StateChange>(data) {
        let _ = serde_json::to_vec(&sc);
    }
    if let Ok(ws_req) = serde_json::from_slice::<jmap_proto::websocket::WebSocketRequest>(data) {
        let _ = serde_json::to_vec(&ws_req);
    }
    if let Ok(ws_resp) = serde_json::from_slice::<jmap_proto::websocket::WebSocketResponse>(data) {
        let _ = serde_json::to_vec(&ws_resp);
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = jmap_proto::text::caret_decoded(s);
        let _ = jmap_proto::text::caret_encoded(s);
        let _ = jmap_proto::text::split_uri(s);
        let _ = jmap_proto::text::fold_overlong_lines(s.to_string());
        let _ = jmap_proto::text::is_valid_scheme(s);
    }
});
