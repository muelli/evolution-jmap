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
    if let Ok(req) = serde_json::from_slice::<jmap_proto::request::Request>(data) {
        let _ = serde_json::to_vec(&req);
    }
    if let Ok(inv) = serde_json::from_slice::<jmap_proto::request::Invocation>(data) {
        let _ = serde_json::to_vec(&inv);
    }
    if let Ok(rref) = serde_json::from_slice::<jmap_proto::request::ResultReference>(data) {
        let _ = serde_json::to_vec(&rref);
    }
    if let Ok(err) = serde_json::from_slice::<jmap_proto::error::RequestError>(data) {
        let _ = serde_json::to_vec(&err);
    }
    if let Ok(err) = serde_json::from_slice::<jmap_proto::error::MethodError>(data) {
        let _ = serde_json::to_vec(&err);
    }
    if let Ok(err) = serde_json::from_slice::<jmap_proto::error::SetError>(data) {
        let _ = serde_json::to_vec(&err);
    }
    if let Ok(greq) = serde_json::from_slice::<jmap_proto::methods::GetRequest>(data) {
        let _ = serde_json::to_vec(&greq);
    }
    if let Ok(gresp) =
        serde_json::from_slice::<jmap_proto::methods::GetResponse<serde_json::Value>>(data)
    {
        let _ = serde_json::to_vec(&gresp);
    }
    if let Ok(sreq) =
        serde_json::from_slice::<jmap_proto::methods::SetRequest<serde_json::Value>>(data)
    {
        let _ = serde_json::to_vec(&sreq);
    }
    if let Ok(sresp) =
        serde_json::from_slice::<jmap_proto::methods::SetResponse<serde_json::Value>>(data)
    {
        let _ = serde_json::to_vec(&sresp);
    }
    if let Ok(qreq) =
        serde_json::from_slice::<jmap_proto::methods::QueryRequest<serde_json::Value>>(data)
    {
        let _ = serde_json::to_vec(&qreq);
    }
    if let Ok(qresp) = serde_json::from_slice::<jmap_proto::methods::QueryResponse>(data) {
        let _ = serde_json::to_vec(&qresp);
    }
    if let Ok(creq) = serde_json::from_slice::<jmap_proto::methods::ChangesRequest>(data) {
        let _ = serde_json::to_vec(&creq);
    }
    if let Ok(cresp) = serde_json::from_slice::<jmap_proto::methods::ChangesResponse>(data) {
        let _ = serde_json::to_vec(&cresp);
    }
    if let Ok(qcreq) =
        serde_json::from_slice::<jmap_proto::methods::QueryChangesRequest<serde_json::Value>>(data)
    {
        let _ = serde_json::to_vec(&qcreq);
    }
    if let Ok(qcresp) = serde_json::from_slice::<jmap_proto::methods::QueryChangesResponse>(data) {
        let _ = serde_json::to_vec(&qcresp);
    }
    if let Ok(item) = serde_json::from_slice::<jmap_proto::methods::AddedItem>(data) {
        let _ = serde_json::to_vec(&item);
    }
    if let Ok(comp) = serde_json::from_slice::<jmap_proto::methods::Comparator>(data) {
        let _ = serde_json::to_vec(&comp);
    }
    if let Ok(cpreq) =
        serde_json::from_slice::<jmap_proto::methods::CopyRequest<serde_json::Value>>(data)
    {
        let _ = serde_json::to_vec(&cpreq);
    }
    if let Ok(cpresp) =
        serde_json::from_slice::<jmap_proto::methods::CopyResponse<serde_json::Value>>(data)
    {
        let _ = serde_json::to_vec(&cpresp);
    }
    if let Ok(bcreq) = serde_json::from_slice::<jmap_proto::methods::BlobCopyRequest>(data) {
        let _ = serde_json::to_vec(&bcreq);
    }
    if let Ok(bcresp) = serde_json::from_slice::<jmap_proto::methods::BlobCopyResponse>(data) {
        let _ = serde_json::to_vec(&bcresp);
    }
    if let Ok(cf) = serde_json::from_slice::<jmap_proto::contacts::ContactCardQueryFilter>(data) {
        let _ = serde_json::to_vec(&cf);
    }
    if let Ok(pf) = serde_json::from_slice::<jmap_proto::principals::PrincipalQueryFilter>(data) {
        let _ = serde_json::to_vec(&pf);
    }
    if let Ok(snf) =
        serde_json::from_slice::<jmap_proto::principals::ShareNotificationQueryFilter>(data)
    {
        let _ = serde_json::to_vec(&snf);
    }
    if let Ok(cef) = serde_json::from_slice::<jmap_proto::calendars::CalendarEventQueryFilter>(data)
    {
        let _ = serde_json::to_vec(&cef);
    }
    if let Ok(mc) = serde_json::from_slice::<jmap_proto::metadata::MetadataCapability>(data) {
        let _ = serde_json::to_vec(&mc);
    }
    if let Ok(dtm) = serde_json::from_slice::<jmap_proto::metadata::DataTypeMetadataInfo>(data) {
        let _ = serde_json::to_vec(&dtm);
    }
    if let Ok(mfc) = serde_json::from_slice::<jmap_proto::metadata::MetadataFilterCondition>(data) {
        let _ = serde_json::to_vec(&mfc);
    }
    if let Ok(mtf) = serde_json::from_slice::<jmap_proto::metadata::MetadataTextFilter>(data) {
        let _ = serde_json::to_vec(&mtf);
    }
    if let Ok(st) = serde_json::from_slice::<jmap_proto::state::State>(data) {
        let _ = serde_json::to_vec(&st);
    }
    if let Ok(ud) = serde_json::from_slice::<jmap_proto::state::UtcDate>(data) {
        let _ = serde_json::to_vec(&ud);
    }
    if let Ok(id) = serde_json::from_slice::<jmap_proto::id::Id>(data) {
        let _ = serde_json::to_vec(&id);
    }
    if let Ok(eqf) = serde_json::from_slice::<jmap_proto::mail::EmailQueryFilter>(data) {
        let _ = serde_json::to_vec(&eqf);
    }
    if let Ok(esqf) =
        serde_json::from_slice::<jmap_proto::mail::EmailSubmissionQueryFilter>(data)
    {
        let _ = serde_json::to_vec(&esqf);
    }
    if let Ok(mcap) = serde_json::from_slice::<jmap_proto::mail::MailCapability>(data) {
        let _ = serde_json::to_vec(&mcap);
    }
    if let Ok(subcap) = serde_json::from_slice::<jmap_proto::mail::SubmissionCapability>(data) {
        let _ = serde_json::to_vec(&subcap);
    }
    if let Ok(mdncap) = serde_json::from_slice::<jmap_proto::mail::MDNCapability>(data) {
        let _ = serde_json::to_vec(&mdncap);
    }
    if let Ok(vac_cap) =
        serde_json::from_slice::<jmap_proto::mail::VacationResponseCapability>(data)
    {
        let _ = serde_json::to_vec(&vac_cap);
    }
    if let Ok(mscap) = serde_json::from_slice::<jmap_proto::mail::MailShareCapability>(data) {
        let _ = serde_json::to_vec(&mscap);
    }
    if let Ok(smimecap) =
        serde_json::from_slice::<jmap_proto::mail::SmimeVerifyCapability>(data)
    {
        let _ = serde_json::to_vec(&smimecap);
    }
    if let Ok(mbr) = serde_json::from_slice::<jmap_proto::mail::MailboxRights>(data) {
        let _ = serde_json::to_vec(&mbr);
    }
    if let Ok(eh) = serde_json::from_slice::<jmap_proto::mail::EmailHeader>(data) {
        let _ = serde_json::to_vec(&eh);
    }
    if let Ok(ea) = serde_json::from_slice::<jmap_proto::mail::EmailAddress>(data) {
        let _ = serde_json::to_vec(&ea);
    }
    if let Ok(eag) = serde_json::from_slice::<jmap_proto::mail::EmailAddressGroup>(data) {
        let _ = serde_json::to_vec(&eag);
    }
    if let Ok(ebp) = serde_json::from_slice::<jmap_proto::mail::EmailBodyPart>(data) {
        let _ = serde_json::to_vec(&ebp);
    }
    if let Ok(ebv) = serde_json::from_slice::<jmap_proto::mail::EmailBodyValue>(data) {
        let _ = serde_json::to_vec(&ebv);
    }
    if let Ok(ds) = serde_json::from_slice::<jmap_proto::mail::DeliveryStatus>(data) {
        let _ = serde_json::to_vec(&ds);
    }
    if let Ok(env) = serde_json::from_slice::<jmap_proto::mail::Envelope>(data) {
        let _ = serde_json::to_vec(&env);
    }
    if let Ok(enva) = serde_json::from_slice::<jmap_proto::mail::EnvelopeAddress>(data) {
        let _ = serde_json::to_vec(&enva);
    }
    if let Ok(sd) = serde_json::from_slice::<jmap_proto::mail::SnoozeDetails>(data) {
        let _ = serde_json::to_vec(&sd);
    }
    if let Ok(eimp) = serde_json::from_slice::<jmap_proto::mail::EmailImport>(data) {
        let _ = serde_json::to_vec(&eimp);
    }
    if let Ok(mdnd) = serde_json::from_slice::<jmap_proto::mail::MDNDisposition>(data) {
        let _ = serde_json::to_vec(&mdnd);
    }
    if let Ok(abr) = serde_json::from_slice::<jmap_proto::contacts::AddressBookRights>(data) {
        let _ = serde_json::to_vec(&abr);
    }
    if let Ok(name) = serde_json::from_slice::<jmap_proto::contacts::Name>(data) {
        let _ = serde_json::to_vec(&name);
    }
    if let Ok(nc) = serde_json::from_slice::<jmap_proto::contacts::NameComponent>(data) {
        let _ = serde_json::to_vec(&nc);
    }
    if let Ok(nn) = serde_json::from_slice::<jmap_proto::contacts::Nickname>(data) {
        let _ = serde_json::to_vec(&nn);
    }
    if let Ok(cenqf) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventNotificationQueryFilter>(data)
    {
        let _ = serde_json::to_vec(&cenqf);
    }
    if let Ok(cr) = serde_json::from_slice::<jmap_proto::calendars::CalendarRights>(data) {
        let _ = serde_json::to_vec(&cr);
    }
    if let Ok(rr) = serde_json::from_slice::<jmap_proto::calendars::RecurrenceRule>(data) {
        let _ = serde_json::to_vec(&rr);
    }
    if let Ok(nday) = serde_json::from_slice::<jmap_proto::calendars::NDay>(data) {
        let _ = serde_json::to_vec(&nday);
    }
    if let Ok(part) = serde_json::from_slice::<jmap_proto::calendars::Participant>(data) {
        let _ = serde_json::to_vec(&part);
    }
    if let Ok(loc) = serde_json::from_slice::<jmap_proto::calendars::Location>(data) {
        let _ = serde_json::to_vec(&loc);
    }
    if let Ok(vloc) = serde_json::from_slice::<jmap_proto::calendars::VirtualLocation>(data) {
        let _ = serde_json::to_vec(&vloc);
    }
    if let Ok(lnk) = serde_json::from_slice::<jmap_proto::calendars::Link>(data) {
        let _ = serde_json::to_vec(&lnk);
    }
    if let Ok(al) = serde_json::from_slice::<jmap_proto::calendars::Alert>(data) {
        let _ = serde_json::to_vec(&al);
    }
    if let Ok(ot) = serde_json::from_slice::<jmap_proto::calendars::OffsetTrigger>(data) {
        let _ = serde_json::to_vec(&ot);
    }
    if let Ok(at) = serde_json::from_slice::<jmap_proto::calendars::AbsoluteTrigger>(data) {
        let _ = serde_json::to_vec(&at);
    }
    if let Ok(er) = serde_json::from_slice::<jmap_proto::calendars::EventRelation>(data) {
        let _ = serde_json::to_vec(&er);
    }
    if let Ok(fbb) = serde_json::from_slice::<jmap_proto::calendars::FreeBusyBlock>(data) {
        let _ = serde_json::to_vec(&fbb);
    }
    if let Ok(calcap) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarsCapability>(data)
    {
        let _ = serde_json::to_vec(&calcap);
    }
    if let Ok(cpcap) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarPreferencesCapability>(data)
    {
        let _ = serde_json::to_vec(&cpcap);
    }
    if let Ok(pp) = serde_json::from_slice::<jmap_proto::calendars::ParticipantProblem>(data) {
        let _ = serde_json::to_vec(&pp);
    }
    if let Ok(prp) = serde_json::from_slice::<jmap_proto::calendars::ParticipantReply>(data) {
        let _ = serde_json::to_vec(&prp);
    }
    if let Ok(sce) = serde_json::from_slice::<jmap_proto::calendars::SendCalendarEvent>(data) {
        let _ = serde_json::to_vec(&sce);
    }
    if let Ok(scer) =
        serde_json::from_slice::<jmap_proto::calendars::SendCalendarEventResult>(data)
    {
        let _ = serde_json::to_vec(&scer);
    }
    if let Ok(sqf) = serde_json::from_slice::<jmap_proto::sieve::SieveScriptQueryFilter>(data) {
        let _ = serde_json::to_vec(&sqf);
    }
    if let Ok(scap) = serde_json::from_slice::<jmap_proto::sieve::SieveCapability>(data) {
        let _ = serde_json::to_vec(&scap);
    }
    if let Ok(tqf) = serde_json::from_slice::<jmap_proto::tasks::TaskQueryFilter>(data) {
        let _ = serde_json::to_vec(&tqf);
    }
    if let Ok(tcap) = serde_json::from_slice::<jmap_proto::tasks::TasksCapability>(data) {
        let _ = serde_json::to_vec(&tcap);
    }
    if let Ok(tlr) = serde_json::from_slice::<jmap_proto::tasks::TaskListRights>(data) {
        let _ = serde_json::to_vec(&tlr);
    }
    if let Ok(pcap) =
        serde_json::from_slice::<jmap_proto::principals::PrincipalsCapability>(data)
    {
        let _ = serde_json::to_vec(&pcap);
    }
    if let Ok(poc) =
        serde_json::from_slice::<jmap_proto::principals::PrincipalsOwnerCapability>(data)
    {
        let _ = serde_json::to_vec(&poc);
    }
    if let Ok(pac) =
        serde_json::from_slice::<jmap_proto::principals::PrincipalAvailabilityCapability>(data)
    {
        let _ = serde_json::to_vec(&pac);
    }
    if let Ok(bp) = serde_json::from_slice::<jmap_proto::principals::BusyPeriod>(data) {
        let _ = serde_json::to_vec(&bp);
    }
    if let Ok(fnqf) = serde_json::from_slice::<jmap_proto::filenode::FileNodeQueryFilter>(data) {
        let _ = serde_json::to_vec(&fnqf);
    }
    if let Ok(fncap) = serde_json::from_slice::<jmap_proto::filenode::FileNodeCapability>(data) {
        let _ = serde_json::to_vec(&fncap);
    }
    if let Ok(fnr) = serde_json::from_slice::<jmap_proto::filenode::FileNodeRights>(data) {
        let _ = serde_json::to_vec(&fnr);
    }
    if let Ok(qqf) = serde_json::from_slice::<jmap_proto::quota::QuotaQueryFilter>(data) {
        let _ = serde_json::to_vec(&qqf);
    }
    if let Ok(qcap) = serde_json::from_slice::<jmap_proto::quota::QuotaCapability>(data) {
        let _ = serde_json::to_vec(&qcap);
    }
    if let Ok(bcap) = serde_json::from_slice::<jmap_proto::blob::BlobCapability>(data) {
        let _ = serde_json::to_vec(&bcap);
    }
    if let Ok(bi) = serde_json::from_slice::<jmap_proto::blob::BlobInfo>(data) {
        let _ = serde_json::to_vec(&bi);
    }
    if let Ok(ub) = serde_json::from_slice::<jmap_proto::blob::UploadBlob>(data) {
        let _ = serde_json::to_vec(&ub);
    }
    if let Ok(ubr) = serde_json::from_slice::<jmap_proto::blob::UploadBlobResult>(data) {
        let _ = serde_json::to_vec(&ubr);
    }
    if let Ok(ds) = serde_json::from_slice::<jmap_proto::blob::DataSource>(data) {
        let _ = serde_json::to_vec(&ds);
    }
    if let Ok(blm) = serde_json::from_slice::<jmap_proto::blob::BlobLookupMatch>(data) {
        let _ = serde_json::to_vec(&blm);
    }
    if let Ok(rpc) = serde_json::from_slice::<jmap_proto::session::RefPlusCapability>(data) {
        let _ = serde_json::to_vec(&rpc);
    }
    if let Ok(wsc) = serde_json::from_slice::<jmap_proto::session::WebSocketCapability>(data) {
        let _ = serde_json::to_vec(&wsc);
    }
    if let Ok(ccap) = serde_json::from_slice::<jmap_proto::session::CoreCapability>(data) {
        let _ = serde_json::to_vec(&ccap);
    }
    if let Ok(acc) = serde_json::from_slice::<jmap_proto::session::Account>(data) {
        let _ = serde_json::to_vec(&acc);
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = jmap_proto::text::caret_decoded(s);
        let _ = jmap_proto::text::caret_encoded(s);
        let _ = jmap_proto::text::split_uri(s);
        let _ = jmap_proto::text::fold_overlong_lines(s.to_string());
        let _ = jmap_proto::text::is_valid_scheme(s);
        let _ = jmap_proto::id::split_shared_identity(s);
    }
});
