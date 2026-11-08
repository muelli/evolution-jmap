// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable smoke tests for session, response, changes, and queryChanges fuzz targets.

use jmap_proto::error::{MethodError, RequestError, SetError};
use jmap_proto::methods::{
    AddedItem, ChangesRequest, ChangesResponse, QueryChangesRequest, QueryChangesResponse,
};
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;
use jmap_proto::session::Session;

#[test]
fn hostile_bytes_do_not_panic_session_or_response_or_changes() {
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
        b"{\"accountId\": null, \"oldState\": 123}",
        b"{\"accountId\": \"acc\", \"oldQueryState\": false}",
    ];

    for input in hostile_inputs {
        if let Ok(session) = serde_json::from_slice::<Session>(input) {
            let _ = serde_json::to_vec(&session);
        }
        if let Ok(resp) = serde_json::from_slice::<Response>(input) {
            let _ = serde_json::to_vec(&resp);
        }
        if let Ok(changes) = serde_json::from_slice::<ChangesResponse>(input) {
            let _ = serde_json::to_vec(&changes);
        }
        if let Ok(qc) = serde_json::from_slice::<QueryChangesResponse>(input) {
            let _ = serde_json::to_vec(&qc);
        }
        if let Ok(req) = serde_json::from_slice::<ChangesRequest>(input) {
            let _ = serde_json::to_vec(&req);
        }
        if let Ok(qreq) = serde_json::from_slice::<QueryChangesRequest<serde_json::Value>>(input) {
            let _ = serde_json::to_vec(&qreq);
        }
        if let Ok(item) = serde_json::from_slice::<AddedItem>(input) {
            let _ = serde_json::to_vec(&item);
        }
        if let Ok(inv) = serde_json::from_slice::<Invocation>(input) {
            let _ = inv.is_error();
            let _ = inv.parse::<MethodError>();
            let _ = serde_json::to_vec(&inv);
        }
        if let Ok(req) = serde_json::from_slice::<Request>(input) {
            let _ = serde_json::to_vec(&req);
        }
        if let Ok(rref) = serde_json::from_slice::<ResultReference>(input) {
            let _ = serde_json::to_vec(&rref);
        }
        if let Ok(err) = serde_json::from_slice::<RequestError>(input) {
            let _ = serde_json::to_vec(&err);
        }
        if let Ok(err) = serde_json::from_slice::<MethodError>(input) {
            let _ = serde_json::to_vec(&err);
        }
        if let Ok(err) = serde_json::from_slice::<SetError>(input) {
            let _ = serde_json::to_vec(&err);
        }
    }
}

#[test]
fn changes_and_query_changes_seeds_roundtrip() {
    let changes_json = br#"{
        "accountId": "acc1",
        "oldState": "state1",
        "newState": "state2",
        "hasMoreChanges": false,
        "created": ["id1", "id2"],
        "updated": ["id3"],
        "destroyed": ["id4"]
    }"#;
    let chg: ChangesResponse =
        serde_json::from_slice(changes_json).expect("ChangesResponse parses");
    assert_eq!(chg.account_id.as_str(), "acc1");
    let serialized = serde_json::to_vec(&chg).expect("ChangesResponse serializes");
    assert!(serde_json::from_slice::<ChangesResponse>(&serialized).is_ok());

    let query_changes_json = br#"{
        "accountId": "acc1",
        "oldQueryState": "qs1",
        "newQueryState": "qs2",
        "total": 5,
        "removed": ["id1"],
        "added": [{"id": "id2", "index": 0}]
    }"#;
    let qc: QueryChangesResponse =
        serde_json::from_slice(query_changes_json).expect("QueryChangesResponse parses");
    assert_eq!(qc.account_id.as_str(), "acc1");
    let serialized_qc = serde_json::to_vec(&qc).expect("QueryChangesResponse serializes");
    assert!(serde_json::from_slice::<QueryChangesResponse>(&serialized_qc).is_ok());
}

#[test]
fn changes_and_query_changes_requests_smoke() {
    let changes_request_json = br#"{
        "accountId": "acc1",
        "sinceState": "state1",
        "maxChanges": 100
    }"#;
    let req: ChangesRequest =
        serde_json::from_slice(changes_request_json).expect("ChangesRequest parses");
    assert_eq!(req.account_id.as_str(), "acc1");
    assert_eq!(req.since_state.as_str(), "state1");
    assert_eq!(req.max_changes, Some(100));

    let built_req =
        ChangesRequest::new(req.account_id.clone(), req.since_state.clone()).max_changes(50);
    assert_eq!(built_req.max_changes, Some(50));
    let serialized_req = serde_json::to_vec(&req).expect("ChangesRequest serializes");
    assert!(serde_json::from_slice::<ChangesRequest>(&serialized_req).is_ok());

    let query_changes_request_json = br#"{
        "accountId": "acc1",
        "sinceQueryState": "qstate1",
        "maxChanges": 25,
        "upToId": "id42",
        "calculateTotal": true,
        "collapseThreads": true
    }"#;
    let qreq: QueryChangesRequest<serde_json::Value> =
        serde_json::from_slice(query_changes_request_json).expect("QueryChangesRequest parses");
    assert_eq!(qreq.account_id.as_str(), "acc1");
    assert_eq!(qreq.since_query_state.as_str(), "qstate1");
    assert_eq!(qreq.max_changes, Some(25));
    assert_eq!(qreq.up_to_id.as_ref().map(|id| id.as_str()), Some("id42"));
    assert!(qreq.calculate_total);
    assert!(qreq.collapse_threads);

    let built_qreq: QueryChangesRequest<serde_json::Value> =
        QueryChangesRequest::new(qreq.account_id.clone(), qreq.since_query_state.clone())
            .max_changes(10)
            .up_to_id("id10")
            .calculate_total()
            .collapse_threads();
    assert_eq!(built_qreq.max_changes, Some(10));
    assert!(built_qreq.calculate_total);
    let serialized_qreq = serde_json::to_vec(&qreq).expect("QueryChangesRequest serializes");
    assert!(
        serde_json::from_slice::<QueryChangesRequest<serde_json::Value>>(&serialized_qreq).is_ok()
    );

    let item_json = br#"{"id": "item1", "index": 3}"#;
    let item: AddedItem = serde_json::from_slice(item_json).expect("AddedItem parses");
    assert_eq!(item.id.as_str(), "item1");
    assert_eq!(item.index, 3);
    let built_item = AddedItem::new("item2", 4);
    assert_eq!(built_item.id.as_str(), "item2");
    assert_eq!(built_item.index, 4);
    let serialized_item = serde_json::to_vec(&item).expect("AddedItem serializes");
    assert!(serde_json::from_slice::<AddedItem>(&serialized_item).is_ok());
}

#[test]
fn response_and_invocation_and_errors_smoke() {
    let response_json = br##"{
        "methodResponses": [
            ["Core/echo", {"arg": "value"}, "c1"],
            ["error", {"type": "unknownMethod", "description": "not found"}, "c2"]
        ],
        "createdIds": {"#1": "real-id"},
        "sessionState": "state1"
    }"##;
    let resp: Response = serde_json::from_slice(response_json).expect("Response parses");
    assert_eq!(resp.responses_for("c1").count(), 1);
    assert_eq!(resp.responses_for("c2").count(), 1);
    assert_eq!(resp.responses_for("c3").count(), 0);

    for invocation in &resp.method_responses {
        if invocation.is_error() {
            let err: MethodError = invocation.parse().expect("MethodError parses");
            assert_eq!(err.error_type, "unknownMethod");
            assert_eq!(err.description.as_deref(), Some("not found"));
        }
    }

    let serialized = serde_json::to_vec(&resp).expect("Response serializes");
    let resp2: Response = serde_json::from_slice(&serialized).expect("Response roundtrips");
    assert_eq!(resp, resp2);

    let req_err_json =
        br#"{"type": "urn:ietf:params:jmap:error:limit", "status": 400, "limit": "maxSizeRequest"}"#;
    let req_err: RequestError = serde_json::from_slice(req_err_json).expect("RequestError parses");
    assert_eq!(req_err.status, Some(400));
    assert_eq!(req_err.error_type, "urn:ietf:params:jmap:error:limit");
    let serialized_req = serde_json::to_vec(&req_err).expect("RequestError serializes");
    assert!(serde_json::from_slice::<RequestError>(&serialized_req).is_ok());

    let set_err_json = br#"{"type": "invalidProperties", "properties": ["name"]}"#;
    let set_err: SetError = serde_json::from_slice(set_err_json).expect("SetError parses");
    assert_eq!(
        set_err.properties.as_deref(),
        Some(&["name".to_string()][..])
    );
    let serialized_set = serde_json::to_vec(&set_err).expect("SetError serializes");
    assert!(serde_json::from_slice::<SetError>(&serialized_set).is_ok());

    let req_json =
        br#"{"using": ["urn:ietf:params:jmap:core"], "methodCalls": [["Core/echo", {}, "c1"]]}"#;
    let req: Request = serde_json::from_slice(req_json).expect("Request parses");
    assert_eq!(req.method_calls.len(), 1);
    let serialized_request = serde_json::to_vec(&req).expect("Request serializes");
    assert!(serde_json::from_slice::<Request>(&serialized_request).is_ok());

    let ref_json = br#"{"resultOf": "c1", "name": "ids", "path": "/0"}"#;
    let rref: ResultReference = serde_json::from_slice(ref_json).expect("ResultReference parses");
    assert_eq!(rref.result_of, "c1");
    let serialized_ref = serde_json::to_vec(&rref).expect("ResultReference serializes");
    assert!(serde_json::from_slice::<ResultReference>(&serialized_ref).is_ok());
}

#[test]
fn hostile_bytes_do_not_panic_expanded_proto_parsers() {
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
        b"{\"name\": null}",
        b"{\"values\": [1, 2, 3]}",
        b"mailto:user@example.com?body=^n^n^",
        b"https://example.com/path?arg=val#fragment",
        b"\xFF\xFE\x00\x00",
        b"This is an overlong text line that needs folding into multiple 75-octet segments without split.",
    ];

    for input in hostile_inputs {
        if let Ok(ab) = serde_json::from_slice::<jmap_proto::contacts::AddressBook>(input) {
            let _ = serde_json::to_vec(&ab);
        }
        if let Ok(cal) = serde_json::from_slice::<jmap_proto::calendars::Calendar>(input) {
            let _ = serde_json::to_vec(&cal);
        }
        if let Ok(pi) = serde_json::from_slice::<jmap_proto::calendars::ParticipantIdentity>(input)
        {
            let _ = serde_json::to_vec(&pi);
        }
        if let Ok(cen) =
            serde_json::from_slice::<jmap_proto::calendars::CalendarEventNotification>(input)
        {
            let _ = serde_json::to_vec(&cen);
        }
        if let Ok(task) = serde_json::from_slice::<jmap_proto::tasks::Task>(input) {
            let _ = serde_json::to_vec(&task);
        }
        if let Ok(tl) = serde_json::from_slice::<jmap_proto::tasks::TaskList>(input) {
            let _ = serde_json::to_vec(&tl);
        }
        if let Ok(pr) = serde_json::from_slice::<jmap_proto::principals::Principal>(input) {
            let _ = serde_json::to_vec(&pr);
        }
        if let Ok(sn) = serde_json::from_slice::<jmap_proto::principals::ShareNotification>(input) {
            let _ = serde_json::to_vec(&sn);
        }
        if let Ok(identity) = serde_json::from_slice::<jmap_proto::mail::Identity>(input) {
            let _ = serde_json::to_vec(&identity);
        }
        if let Ok(sub) = serde_json::from_slice::<jmap_proto::mail::EmailSubmission>(input) {
            let _ = serde_json::to_vec(&sub);
        }
        if let Ok(th) = serde_json::from_slice::<jmap_proto::mail::Thread>(input) {
            let _ = serde_json::to_vec(&th);
        }
        if let Ok(snip) = serde_json::from_slice::<jmap_proto::mail::SearchSnippet>(input) {
            let _ = serde_json::to_vec(&snip);
        }
        if let Ok(script) = serde_json::from_slice::<jmap_proto::sieve::SieveScript>(input) {
            let _ = serde_json::to_vec(&script);
        }
        if let Ok(quota) = serde_json::from_slice::<jmap_proto::quota::Quota>(input) {
            let _ = serde_json::to_vec(&quota);
        }
        if let Ok(fn_node) = serde_json::from_slice::<jmap_proto::filenode::FileNode>(input) {
            let _ = serde_json::to_vec(&fn_node);
        }
        if let Ok(blob_up) = serde_json::from_slice::<jmap_proto::blob::BlobUploadResponse>(input) {
            let _ = serde_json::to_vec(&blob_up);
        }
        if let Ok(blob_get) = serde_json::from_slice::<jmap_proto::blob::BlobGetResponse>(input) {
            let _ = serde_json::to_vec(&blob_get);
        }
        if let Ok(ps) = serde_json::from_slice::<jmap_proto::push::PushSubscription>(input) {
            let _ = serde_json::to_vec(&ps);
        }
        if let Ok(sc) = serde_json::from_slice::<jmap_proto::push::StateChange>(input) {
            let _ = serde_json::to_vec(&sc);
        }
        if let Ok(ws_req) = serde_json::from_slice::<jmap_proto::websocket::WebSocketRequest>(input)
        {
            let _ = serde_json::to_vec(&ws_req);
        }
        if let Ok(ws_resp) =
            serde_json::from_slice::<jmap_proto::websocket::WebSocketResponse>(input)
        {
            let _ = serde_json::to_vec(&ws_resp);
        }
        if let Ok(cap) = serde_json::from_slice::<jmap_proto::session::CoreCapability>(input) {
            let _ = serde_json::to_vec(&cap);
        }
        if let Ok(ws) = serde_json::from_slice::<jmap_proto::session::WebSocketCapability>(input) {
            let _ = serde_json::to_vec(&ws);
        }
        if let Ok(rp) = serde_json::from_slice::<jmap_proto::session::RefPlusCapability>(input) {
            let _ = serde_json::to_vec(&rp);
        }
        if let Ok(mail) = serde_json::from_slice::<jmap_proto::mail::MailCapability>(input) {
            let _ = serde_json::to_vec(&mail);
        }
        if let Ok(sub) = serde_json::from_slice::<jmap_proto::mail::SubmissionCapability>(input) {
            let _ = serde_json::to_vec(&sub);
        }
        if let Ok(vac) =
            serde_json::from_slice::<jmap_proto::mail::VacationResponseCapability>(input)
        {
            let _ = serde_json::to_vec(&vac);
        }
        if let Ok(mdn) = serde_json::from_slice::<jmap_proto::mail::MDNCapability>(input) {
            let _ = serde_json::to_vec(&mdn);
        }
        if let Ok(contacts) =
            serde_json::from_slice::<jmap_proto::contacts::ContactsCapability>(input)
        {
            let _ = serde_json::to_vec(&contacts);
        }
        if let Ok(cals) =
            serde_json::from_slice::<jmap_proto::calendars::CalendarsCapability>(input)
        {
            let _ = serde_json::to_vec(&cals);
        }
        if let Ok(cal_pref) =
            serde_json::from_slice::<jmap_proto::calendars::CalendarPreferencesCapability>(input)
        {
            let _ = serde_json::to_vec(&cal_pref);
        }
        if let Ok(pr) =
            serde_json::from_slice::<jmap_proto::principals::PrincipalsCapability>(input)
        {
            let _ = serde_json::to_vec(&pr);
        }
        if let Ok(pr_owner) =
            serde_json::from_slice::<jmap_proto::principals::PrincipalsOwnerCapability>(input)
        {
            let _ = serde_json::to_vec(&pr_owner);
        }
        if let Ok(pr_avail) =
            serde_json::from_slice::<jmap_proto::principals::PrincipalAvailabilityCapability>(input)
        {
            let _ = serde_json::to_vec(&pr_avail);
        }
        if let Ok(q) = serde_json::from_slice::<jmap_proto::quota::QuotaCapability>(input) {
            let _ = serde_json::to_vec(&q);
        }
        if let Ok(b) = serde_json::from_slice::<jmap_proto::blob::BlobCapability>(input) {
            let _ = serde_json::to_vec(&b);
        }
        if let Ok(t) = serde_json::from_slice::<jmap_proto::tasks::TasksCapability>(input) {
            let _ = serde_json::to_vec(&t);
        }
        if let Ok(s) = serde_json::from_slice::<jmap_proto::sieve::SieveCapability>(input) {
            let _ = serde_json::to_vec(&s);
        }
        if let Ok(sm) = serde_json::from_slice::<jmap_proto::mail::SmimeVerifyCapability>(input) {
            let _ = serde_json::to_vec(&sm);
        }
        if let Ok(fn_cap) =
            serde_json::from_slice::<jmap_proto::filenode::FileNodeCapability>(input)
        {
            let _ = serde_json::to_vec(&fn_cap);
        }
        if let Ok(meta) = serde_json::from_slice::<jmap_proto::metadata::MetadataCapability>(input)
        {
            let _ = serde_json::to_vec(&meta);
        }
        if let Ok(ms) = serde_json::from_slice::<jmap_proto::mail::MailShareCapability>(input) {
            let _ = serde_json::to_vec(&ms);
        }
        if let Ok(vapid) = serde_json::from_slice::<jmap_proto::push::WebPushVapidCapability>(input)
        {
            let _ = serde_json::to_vec(&vapid);
        }
        if let Ok(s) = std::str::from_utf8(input) {
            let _ = jmap_proto::text::caret_decoded(s);
            let _ = jmap_proto::text::caret_encoded(s);
            let _ = jmap_proto::text::split_uri(s);
            let _ = jmap_proto::text::fold_overlong_lines(s.to_string());
            let _ = jmap_proto::text::is_valid_scheme(s);
        }
    }
}

#[test]
fn session_all_capabilities_and_builders_smoke() {
    use jmap_proto::session::*;
    use std::collections::BTreeMap;

    let core_cap = CoreCapability::new()
        .with_max_size_upload(10_000_000)
        .with_max_concurrent_upload(4)
        .with_max_size_request(20_000_000)
        .with_max_concurrent_requests(8)
        .with_max_calls_in_request(16)
        .with_max_objects_in_get(100)
        .with_max_objects_in_set(50)
        .with_collation_algorithms(["i;ascii-casemap", "i;unicode-casemap"])
        .with_extra(BTreeMap::from([(
            "vendorProp".into(),
            serde_json::json!(true),
        )]));
    assert_eq!(core_cap.max_size_upload, 10_000_000);
    assert_eq!(core_cap.collation_algorithms.len(), 2);
    let core_json = serde_json::to_vec(&core_cap).expect("CoreCapability serializes");
    let core_deser: CoreCapability =
        serde_json::from_slice(&core_json).expect("CoreCapability deserializes");
    assert_eq!(core_deser.max_concurrent_requests, 8);

    let ws_cap = WebSocketCapability::new("wss://jmap.example.com/ws")
        .supports_push(true)
        .with_extra(BTreeMap::from([("custom".into(), serde_json::json!(42))]));
    assert!(ws_cap.supports_push);
    let ws_json = serde_json::to_vec(&ws_cap).expect("WebSocketCapability serializes");
    let ws_deser: WebSocketCapability =
        serde_json::from_slice(&ws_json).expect("WebSocketCapability deserializes");
    assert_eq!(ws_deser.url, "wss://jmap.example.com/ws");

    let rp_cap = RefPlusCapability::new()
        .with_json_path(true)
        .with_filter_condition(false)
        .with_set_property(true)
        .with_extra(BTreeMap::new());
    assert_eq!(rp_cap.json_path, Some(true));
    assert_eq!(rp_cap.filter_condition, Some(false));
    let rp_json = serde_json::to_vec(&rp_cap).expect("RefPlusCapability serializes");
    let rp_deser: RefPlusCapability =
        serde_json::from_slice(&rp_json).expect("RefPlusCapability deserializes");
    assert_eq!(rp_deser.set_property, Some(true));

    let mail_cap = jmap_proto::mail::MailCapability::new()
        .with_max_size_attachments_per_email(25_000_000)
        .with_max_size_email_in_bytes(50_000_000)
        .with_max_size_body_value_bytes(1_000_000)
        .with_max_number_of_attachments_per_email(100)
        .with_max_number_of_recipients_per_email(50)
        .may_create_top_level_mailbox(true)
        .with_extra(BTreeMap::new());
    assert_eq!(mail_cap.max_size_attachments_per_email, 25_000_000);
    assert!(mail_cap.may_create_top_level_mailbox);
    let mail_json = serde_json::to_vec(&mail_cap).expect("MailCapability serializes");
    let mail_deser: jmap_proto::mail::MailCapability =
        serde_json::from_slice(&mail_json).expect("MailCapability deserializes");
    assert_eq!(mail_deser.max_size_email_in_bytes, 50_000_000);

    let sub_cap = jmap_proto::mail::SubmissionCapability::new()
        .with_max_delayed_send(86400)
        .with_submission_extensions(BTreeMap::from([(
            "FUTURERELEASE".into(),
            vec!["86400".into()],
        )]))
        .with_extra(BTreeMap::new());
    assert_eq!(sub_cap.max_delayed_send, 86400);
    let sub_json = serde_json::to_vec(&sub_cap).expect("SubmissionCapability serializes");
    let sub_deser: jmap_proto::mail::SubmissionCapability =
        serde_json::from_slice(&sub_json).expect("SubmissionCapability deserializes");
    assert_eq!(sub_deser.max_delayed_send, 86400);

    let contacts_cap = jmap_proto::contacts::ContactsCapability::new()
        .with_max_size_attachments_per_card(10_000_000)
        .with_max_number_of_cards_in_set(200)
        .with_extra(BTreeMap::new());
    assert_eq!(contacts_cap.max_size_attachments_per_card, 10_000_000);
    let contacts_json = serde_json::to_vec(&contacts_cap).expect("ContactsCapability serializes");
    let contacts_deser: jmap_proto::contacts::ContactsCapability =
        serde_json::from_slice(&contacts_json).expect("ContactsCapability deserializes");
    assert_eq!(contacts_deser.max_number_of_cards_in_set, 200);

    let cal_cap = jmap_proto::calendars::CalendarsCapability::new()
        .with_max_calendars_per_event(5)
        .with_min_date_time("1970-01-01T00:00:00")
        .with_max_date_time("2099-12-31T23:59:59")
        .with_max_expanded_query_duration("P365D")
        .with_max_participants_per_event(100)
        .with_may_create_calendar(true)
        .with_extra(BTreeMap::new());
    assert_eq!(cal_cap.max_calendars_per_event, Some(5));
    let cal_json = serde_json::to_vec(&cal_cap).expect("CalendarsCapability serializes");
    let cal_deser: jmap_proto::calendars::CalendarsCapability =
        serde_json::from_slice(&cal_json).expect("CalendarsCapability deserializes");
    assert_eq!(cal_deser.max_participants_per_event, Some(100));

    let pr_cap = jmap_proto::principals::PrincipalsCapability::new()
        .with_current_user_principal_id(jmap_proto::id::Id::from("p123"))
        .with_extra(BTreeMap::new());
    assert_eq!(
        pr_cap.current_user_principal_id.as_ref(),
        Some(&jmap_proto::id::Id::from("p123"))
    );
    let pr_json = serde_json::to_vec(&pr_cap).expect("PrincipalsCapability serializes");
    let pr_deser: jmap_proto::principals::PrincipalsCapability =
        serde_json::from_slice(&pr_json).expect("PrincipalsCapability deserializes");
    assert_eq!(
        pr_deser.current_user_principal_id.as_ref(),
        Some(&jmap_proto::id::Id::from("p123"))
    );

    let blob_cap = jmap_proto::blob::BlobCapability::new()
        .with_max_size_blob_set(1_000_000)
        .with_max_data_sources(10)
        .with_supported_type_names(["image/png", "text/plain"])
        .with_supported_digest_algorithms(["sha-256"])
        .with_extra(BTreeMap::new());
    assert_eq!(blob_cap.max_size_blob_set, Some(1_000_000));
    let blob_json = serde_json::to_vec(&blob_cap).expect("BlobCapability serializes");
    let blob_deser: jmap_proto::blob::BlobCapability =
        serde_json::from_slice(&blob_json).expect("BlobCapability deserializes");
    assert_eq!(blob_deser.max_data_sources, Some(10));

    let tasks_cap = jmap_proto::tasks::TasksCapability::new()
        .with_max_tasks_per_get(250)
        .with_max_tasks_per_set(50)
        .with_max_task_lists_per_get(10)
        .with_extra(BTreeMap::new());
    assert_eq!(tasks_cap.max_tasks_per_get, Some(250));
    let tasks_json = serde_json::to_vec(&tasks_cap).expect("TasksCapability serializes");
    let tasks_deser: jmap_proto::tasks::TasksCapability =
        serde_json::from_slice(&tasks_json).expect("TasksCapability deserializes");
    assert_eq!(tasks_deser.max_tasks_per_set, Some(50));

    let sieve_cap = jmap_proto::sieve::SieveCapability::new(1024 * 1024)
        .with_max_size_script_name(256)
        .with_max_number_scripts(100)
        .with_implementation("Cyrus Sieve v3")
        .with_sieve_extensions(["fileinto", "reject"])
        .with_extra(BTreeMap::new());
    assert_eq!(sieve_cap.max_size_script, 1024 * 1024);
    assert_eq!(sieve_cap.max_size_script_name, Some(256));
    let sieve_json = serde_json::to_vec(&sieve_cap).expect("SieveCapability serializes");
    let sieve_deser: jmap_proto::sieve::SieveCapability =
        serde_json::from_slice(&sieve_json).expect("SieveCapability deserializes");
    assert_eq!(
        sieve_deser.implementation.as_deref(),
        Some("Cyrus Sieve v3")
    );

    let fn_cap = jmap_proto::filenode::FileNodeCapability::new()
        .with_max_file_node_depth(10)
        .with_max_size_file_node_name(255)
        .with_extra(BTreeMap::new());
    assert_eq!(fn_cap.max_file_node_depth, Some(10));
    let fn_json = serde_json::to_vec(&fn_cap).expect("FileNodeCapability serializes");
    let fn_deser: jmap_proto::filenode::FileNodeCapability =
        serde_json::from_slice(&fn_json).expect("FileNodeCapability deserializes");
    assert_eq!(fn_deser.max_size_file_node_name, Some(255));

    let meta_cap = jmap_proto::metadata::MetadataCapability::new()
        .with_data_type(
            "Mailbox",
            jmap_proto::metadata::DataTypeMetadataInfo::new()
                .with_namespaces(["/shared", "/private"])
                .supports_vendor_namespaces(true)
                .supports_private(true)
                .with_max_depth(8),
        )
        .with_extra(BTreeMap::new());
    assert_eq!(meta_cap.data_types.len(), 1);
    let meta_json = serde_json::to_vec(&meta_cap).expect("MetadataCapability serializes");
    let meta_deser: jmap_proto::metadata::MetadataCapability =
        serde_json::from_slice(&meta_json).expect("MetadataCapability deserializes");
    assert!(meta_deser.data_types.contains_key("Mailbox"));

    let vapid_cap = jmap_proto::push::WebPushVapidCapability::new("BEl62iUYgUivxIkv")
        .with_application_server_key("BEl62iUYgUivxIkvKey2")
        .with_extra(BTreeMap::new());
    assert_eq!(vapid_cap.application_server_key, "BEl62iUYgUivxIkvKey2");
    let vapid_json = serde_json::to_vec(&vapid_cap).expect("WebPushVapidCapability serializes");
    let vapid_deser: jmap_proto::push::WebPushVapidCapability =
        serde_json::from_slice(&vapid_json).expect("WebPushVapidCapability deserializes");
    assert_eq!(vapid_deser.application_server_key, "BEl62iUYgUivxIkvKey2");

    let account_id = jmap_proto::id::Id::from("acc-1");
    let account = Account::new("Primary Account")
        .is_personal(true)
        .is_read_only(false)
        .with_capability(CAPABILITY_CORE, serde_json::json!({}))
        .with_capability(CAPABILITY_MAIL, serde_json::to_value(&mail_cap).unwrap())
        .with_capability(
            CAPABILITY_SUBMISSION,
            serde_json::to_value(&sub_cap).unwrap(),
        )
        .with_capability(
            CAPABILITY_CONTACTS,
            serde_json::to_value(&contacts_cap).unwrap(),
        )
        .with_capability(
            CAPABILITY_CALENDARS,
            serde_json::to_value(&cal_cap).unwrap(),
        )
        .with_capability(
            CAPABILITY_PRINCIPALS,
            serde_json::to_value(&pr_cap).unwrap(),
        )
        .with_extra(BTreeMap::from([(
            "customAcc".into(),
            serde_json::json!("val"),
        )]));
    assert!(account.is_personal);
    assert!(!account.is_read_only);
    assert!(account.has_capability(CAPABILITY_CORE));
    assert!(account.has_capability(CAPABILITY_MAIL));
    assert!(account.has_capability(CAPABILITY_SUBMISSION));
    assert_eq!(account.max_delayed_send(), Some(86400));
    assert!(account.submission_capability().is_some());

    let session = Session::new(
        "user@example.com",
        "https://api.example.com",
        "https://download.example.com",
        "https://upload.example.com",
        "state-1234",
    )
    .with_capability(CAPABILITY_CORE, serde_json::to_value(&core_cap).unwrap())
    .with_capability(CAPABILITY_MAIL, serde_json::to_value(&mail_cap).unwrap())
    .with_capability(
        CAPABILITY_SUBMISSION,
        serde_json::to_value(&sub_cap).unwrap(),
    )
    .with_capability(
        CAPABILITY_CONTACTS,
        serde_json::to_value(&contacts_cap).unwrap(),
    )
    .with_capability(
        CAPABILITY_CALENDARS,
        serde_json::to_value(&cal_cap).unwrap(),
    )
    .with_capability(
        CAPABILITY_PRINCIPALS,
        serde_json::to_value(&pr_cap).unwrap(),
    )
    .with_capability(CAPABILITY_WEBSOCKET, serde_json::to_value(&ws_cap).unwrap())
    .with_capability(CAPABILITY_REFPLUS, serde_json::to_value(&rp_cap).unwrap())
    .with_capability(CAPABILITY_BLOB, serde_json::to_value(&blob_cap).unwrap())
    .with_capability(CAPABILITY_TASKS, serde_json::to_value(&tasks_cap).unwrap())
    .with_capability(CAPABILITY_SIEVE, serde_json::to_value(&sieve_cap).unwrap())
    .with_capability(CAPABILITY_FILENODE, serde_json::to_value(&fn_cap).unwrap())
    .with_capability(
        CAPABILITY_METADATA,
        serde_json::to_value(&meta_cap).unwrap(),
    )
    .with_capability(
        CAPABILITY_WEBPUSH_VAPID,
        serde_json::to_value(&vapid_cap).unwrap(),
    )
    .with_account(account_id.clone(), account.clone())
    .with_primary_account(CAPABILITY_MAIL, account_id.clone())
    .with_event_source_url("https://events.example.com/sse")
    .with_extra(BTreeMap::from([(
        "serverVendor".into(),
        serde_json::json!("Test"),
    )]));

    assert_eq!(session.username, "user@example.com");
    assert_eq!(session.event_source_url, "https://events.example.com/sse");
    assert_eq!(session.primary_account(CAPABILITY_MAIL), Some(&account_id));
    assert_eq!(
        session.resolve_primary_account(CAPABILITY_MAIL),
        Some(&account_id)
    );
    assert_eq!(
        session.resolve_primary_account(CAPABILITY_CORE),
        Some(&account_id)
    );
    assert_eq!(session.max_size_upload(), Some(10_000_000));
    assert_eq!(session.max_concurrent_requests(), Some(8));
    assert_eq!(session.collation_algorithms().unwrap().len(), 2);

    assert!(session.core_capability().is_some());
    assert!(session.mail_capability().is_some());
    assert!(session.submission_capability().is_some());
    assert!(session.contacts_capability().is_some());
    assert!(session.calendars_capability().is_some());
    assert!(session.principals_capability().is_some());
    assert!(session.websocket_capability().is_some());
    assert!(session.blob_capability().is_some());
    assert!(session.tasks_capability().is_some());
    assert!(session.sieve_capability().is_some());
    assert!(session.filenode_capability().is_some());
    assert!(session.refplus_capability().is_some());
    assert!(session.metadata_capability().is_some());
    assert!(session.webpush_vapid_capability().is_some());

    let session_json = serde_json::to_vec(&session).expect("Session serializes");
    let session_deser: Session =
        serde_json::from_slice(&session_json).expect("Session deserializes");
    assert_eq!(session_deser.username, "user@example.com");
    assert_eq!(session_deser.accounts.len(), 1);
}
