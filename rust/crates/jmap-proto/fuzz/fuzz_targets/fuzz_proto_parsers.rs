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
    if let Ok(sn) = serde_json::from_slice::<jmap_proto::principals::ShareNotification>(data) {
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
    if let Ok(ws_err) = serde_json::from_slice::<jmap_proto::websocket::WebSocketRequestError>(data)
    {
        let _ = serde_json::to_vec(&ws_err);
    }
    if let Ok(ws_pe) = serde_json::from_slice::<jmap_proto::websocket::WebSocketPushEnable>(data) {
        let _ = serde_json::to_vec(&ws_pe);
    }
    if let Ok(ws_pd) = serde_json::from_slice::<jmap_proto::websocket::WebSocketPushDisable>(data) {
        let _ = serde_json::to_vec(&ws_pd);
    }
    if let Ok(vac) = serde_json::from_slice::<jmap_proto::mail::VacationResponse>(data) {
        let _ = serde_json::to_vec(&vac);
    }
    if let Ok(mdn) = serde_json::from_slice::<jmap_proto::mail::MDN>(data) {
        let _ = serde_json::to_vec(&mdn);
    }
    if let Ok(mdn_req) = serde_json::from_slice::<jmap_proto::mail::MDNSendRequest>(data) {
        let _ = serde_json::to_vec(&mdn_req);
    }
    if let Ok(mdn_resp) = serde_json::from_slice::<jmap_proto::mail::MDNSendResponse>(data) {
        let _ = serde_json::to_vec(&mdn_resp);
    }
    if let Ok(imp_req) = serde_json::from_slice::<jmap_proto::mail::EmailImportRequest>(data) {
        let _ = serde_json::to_vec(&imp_req);
    }
    if let Ok(imp_resp) = serde_json::from_slice::<jmap_proto::mail::EmailImportResponse>(data) {
        let _ = serde_json::to_vec(&imp_resp);
    }
    if let Ok(get_req) = serde_json::from_slice::<jmap_proto::mail::EmailGetRequest>(data) {
        let _ = serde_json::to_vec(&get_req);
    }
    if let Ok(parse_req) = serde_json::from_slice::<jmap_proto::mail::EmailParseRequest>(data) {
        let _ = serde_json::to_vec(&parse_req);
    }
    if let Ok(parse_resp) = serde_json::from_slice::<jmap_proto::mail::EmailParseResponse>(data) {
        let _ = serde_json::to_vec(&parse_resp);
    }
    if let Ok(snip_req) = serde_json::from_slice::<jmap_proto::mail::SearchSnippetGetRequest>(data)
    {
        let _ = serde_json::to_vec(&snip_req);
    }
    if let Ok(snip_resp) =
        serde_json::from_slice::<jmap_proto::mail::SearchSnippetGetResponse>(data)
    {
        let _ = serde_json::to_vec(&snip_resp);
    }
    if let Ok(sub_set) = serde_json::from_slice::<jmap_proto::mail::EmailSubmissionSetRequest>(data)
    {
        let _ = serde_json::to_vec(&sub_set);
    }
    if let Ok(ab_set) = serde_json::from_slice::<jmap_proto::contacts::AddressBookSetRequest>(data)
    {
        let _ = serde_json::to_vec(&ab_set);
    }
    if let Ok(cg) = serde_json::from_slice::<jmap_proto::contacts::CardGroup>(data) {
        let _ = serde_json::to_vec(&cg);
    }
    if let Ok(cp_req) =
        serde_json::from_slice::<jmap_proto::contacts::ContactCardParseRequest>(data)
    {
        let _ = serde_json::to_vec(&cp_req);
    }
    if let Ok(cp_resp) =
        serde_json::from_slice::<jmap_proto::contacts::ContactCardParseResponse>(data)
    {
        let _ = serde_json::to_vec(&cp_resp);
    }
    if let Ok(cal_set) = serde_json::from_slice::<jmap_proto::calendars::CalendarSetRequest>(data) {
        let _ = serde_json::to_vec(&cal_set);
    }
    if let Ok(cal_pref) = serde_json::from_slice::<jmap_proto::calendars::CalendarPreferences>(data)
    {
        let _ = serde_json::to_vec(&cal_pref);
    }
    if let Ok(ev_set) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventSetRequest>(data)
    {
        let _ = serde_json::to_vec(&ev_set);
    }
    if let Ok(ev_parse_req) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventParseRequest>(data)
    {
        let _ = serde_json::to_vec(&ev_parse_req);
    }
    if let Ok(ev_parse_resp) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventParseResponse>(data)
    {
        let _ = serde_json::to_vec(&ev_parse_resp);
    }
    if let Ok(fb_req) = serde_json::from_slice::<jmap_proto::calendars::GetFreeBusyRequest>(data) {
        let _ = serde_json::to_vec(&fb_req);
    }
    if let Ok(fb_resp) = serde_json::from_slice::<jmap_proto::calendars::GetFreeBusyResponse>(data)
    {
        let _ = serde_json::to_vec(&fb_resp);
    }
    if let Ok(cal_grp) = serde_json::from_slice::<jmap_proto::calendars::CalendarGroup>(data) {
        let _ = serde_json::to_vec(&cal_grp);
    }
    if let Ok(cal_alert) = serde_json::from_slice::<jmap_proto::calendars::CalendarAlert>(data) {
        let _ = serde_json::to_vec(&cal_alert);
    }
    if let Ok(send_req) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventSendRequest>(data)
    {
        let _ = serde_json::to_vec(&send_req);
    }
    if let Ok(send_resp) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventSendResponse>(data)
    {
        let _ = serde_json::to_vec(&send_resp);
    }
    if let Ok(pi_set) =
        serde_json::from_slice::<jmap_proto::calendars::ParticipantIdentitySetRequest>(data)
    {
        let _ = serde_json::to_vec(&pi_set);
    }
    if let Ok(avail_req) =
        serde_json::from_slice::<jmap_proto::principals::GetAvailabilityRequest>(data)
    {
        let _ = serde_json::to_vec(&avail_req);
    }
    if let Ok(avail_resp) =
        serde_json::from_slice::<jmap_proto::principals::GetAvailabilityResponse>(data)
    {
        let _ = serde_json::to_vec(&avail_resp);
    }
    if let Ok(sieve_set) = serde_json::from_slice::<jmap_proto::sieve::SieveScriptSetRequest>(data)
    {
        let _ = serde_json::to_vec(&sieve_set);
    }
    if let Ok(sieve_val_req) =
        serde_json::from_slice::<jmap_proto::sieve::SieveScriptValidateRequest>(data)
    {
        let _ = serde_json::to_vec(&sieve_val_req);
    }
    if let Ok(sieve_val_resp) =
        serde_json::from_slice::<jmap_proto::sieve::SieveScriptValidateResponse>(data)
    {
        let _ = serde_json::to_vec(&sieve_val_resp);
    }
    if let Ok(blob_up_req) = serde_json::from_slice::<jmap_proto::blob::BlobUploadRequest>(data) {
        let _ = serde_json::to_vec(&blob_up_req);
    }
    if let Ok(blob_get_req) = serde_json::from_slice::<jmap_proto::blob::BlobGetRequest>(data) {
        let _ = serde_json::to_vec(&blob_get_req);
    }
    if let Ok(blob_lk_req) = serde_json::from_slice::<jmap_proto::blob::BlobLookupRequest>(data) {
        let _ = serde_json::to_vec(&blob_lk_req);
    }
    if let Ok(blob_lk_resp) = serde_json::from_slice::<jmap_proto::blob::BlobLookupResponse>(data) {
        let _ = serde_json::to_vec(&blob_lk_resp);
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = jmap_proto::text::caret_decoded(s);
        let _ = jmap_proto::text::caret_encoded(s);
        let _ = jmap_proto::text::split_uri(s);
        let _ = jmap_proto::text::fold_overlong_lines(s.to_string());
        let _ = jmap_proto::text::is_valid_scheme(s);
    }
});
