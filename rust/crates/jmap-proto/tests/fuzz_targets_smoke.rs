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

#[test]
fn response_method_envelopes_and_builders_smoke() {
    use jmap_proto::id::Id;
    use jmap_proto::methods::{
        BlobCopyRequest, BlobCopyResponse, Comparator, CopyRequest, CopyResponse, Echo, Filter,
        GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
        UploadResponse,
    };
    use jmap_proto::state::State;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    let get_req = GetRequest::ids("acc1", ["id1", "id2"])
        .properties(["id", "name"])
        .ids_ref(ResultReference::new("c0", "Foo/query", "/ids"));
    let get_req_bytes = serde_json::to_vec(&get_req).expect("GetRequest serializes");
    let get_req_parsed: GetRequest =
        serde_json::from_slice(&get_req_bytes).expect("GetRequest parses");
    assert_eq!(get_req_parsed, get_req);

    let get_all = GetRequest::all("acc1");
    assert!(get_all.ids.is_none());

    let get_resp = GetResponse::new("acc1", "s1", [json!({"id": "id1"})]).with_not_found(["id2"]);
    let get_resp_bytes = serde_json::to_vec(&get_resp).expect("GetResponse serializes");
    let get_resp_parsed: GetResponse<Value> =
        serde_json::from_slice(&get_resp_bytes).expect("GetResponse parses");
    assert_eq!(get_resp_parsed, get_resp);

    let set_req = SetRequest::new("acc1")
        .if_in_state("s0")
        .create("c1", json!({"name": "Created"}))
        .update("id1", json!({"name": "Updated"}))
        .destroy("id2");
    let set_req_bytes = serde_json::to_vec(&set_req).expect("SetRequest serializes");
    let set_req_parsed: SetRequest<Value> =
        serde_json::from_slice(&set_req_bytes).expect("SetRequest parses");
    assert_eq!(set_req_parsed, set_req);

    let set_err = SetError::new("invalidProperties")
        .with_description("bad prop")
        .with_properties(["name"])
        .with_extra(BTreeMap::from([("vendor".into(), json!(1))]));
    let set_resp = SetResponse::new("acc1", "s2")
        .with_old_state("s1")
        .with_created(BTreeMap::from([("c1".into(), json!({"id": "new1"}))]))
        .with_updated(BTreeMap::from([(Id::from("id1"), None)]))
        .with_destroyed(["id2"])
        .with_not_created(BTreeMap::from([("c2".into(), set_err.clone())]))
        .with_not_updated(BTreeMap::from([(Id::from("id3"), set_err.clone())]))
        .with_not_destroyed(BTreeMap::from([(Id::from("id4"), set_err)]));
    let set_resp_bytes = serde_json::to_vec(&set_resp).expect("SetResponse serializes");
    let set_resp_parsed: SetResponse<Value> =
        serde_json::from_slice(&set_resp_bytes).expect("SetResponse parses");
    assert_eq!(set_resp_parsed, set_resp);

    let f_cond = Filter::condition(json!({"name": "alice"}));
    let f_tree = Filter::and([
        f_cond.clone(),
        Filter::or([f_cond.clone(), Filter::not([f_cond])]),
    ]);
    let q_req = QueryRequest::new("acc1")
        .filter(f_tree)
        .sort([
            Comparator::ascending("name").with_collation("i;unicode-casemap"),
            Comparator::descending("receivedAt"),
        ])
        .position(5)
        .anchor("id1")
        .anchor_offset(-2)
        .limit(25)
        .calculate_total()
        .collapse_threads();
    let q_req_bytes = serde_json::to_vec(&q_req).expect("QueryRequest serializes");
    let q_req_parsed: QueryRequest<Filter<Value>> =
        serde_json::from_slice(&q_req_bytes).expect("QueryRequest parses");
    assert_eq!(q_req_parsed, q_req);

    let q_resp = QueryResponse::new("acc1", "qs1", ["id1", "id2"])
        .with_position(5)
        .with_total(100)
        .with_limit(25)
        .can_calculate_changes(true);
    let q_resp_bytes = serde_json::to_vec(&q_resp).expect("QueryResponse serializes");
    let q_resp_parsed: QueryResponse =
        serde_json::from_slice(&q_resp_bytes).expect("QueryResponse parses");
    assert_eq!(q_resp_parsed, q_resp);

    let copy_req = CopyRequest::new("acc1", "acc2")
        .if_from_in_state("s1")
        .if_in_state("s2")
        .copy_object("c1", json!({"id": "orig1"}))
        .on_success_destroy_original()
        .destroy_from_if_in_state("s1");
    let copy_req_bytes = serde_json::to_vec(&copy_req).expect("CopyRequest serializes");
    let copy_req_parsed: CopyRequest<Value> =
        serde_json::from_slice(&copy_req_bytes).expect("CopyRequest parses");
    assert_eq!(copy_req_parsed, copy_req);

    let copy_resp = CopyResponse::new("acc1", "acc2", "s3")
        .with_old_state("s2")
        .with_created(BTreeMap::from([("c1".into(), json!({"id": "new1"}))]))
        .with_not_created(BTreeMap::from([(
            "c2".into(),
            SetError::new("notFound").with_description("missing"),
        )]));
    let copy_resp_bytes = serde_json::to_vec(&copy_resp).expect("CopyResponse serializes");
    let copy_resp_parsed: CopyResponse<Value> =
        serde_json::from_slice(&copy_resp_bytes).expect("CopyResponse parses");
    assert_eq!(copy_resp_parsed, copy_resp);

    let bcopy_req = BlobCopyRequest::new("acc1", "acc2", ["b1", "b2"]);
    let bcopy_req_bytes = serde_json::to_vec(&bcopy_req).expect("BlobCopyRequest serializes");
    let bcopy_req_parsed: BlobCopyRequest =
        serde_json::from_slice(&bcopy_req_bytes).expect("BlobCopyRequest parses");
    assert_eq!(bcopy_req_parsed, bcopy_req);

    let bcopy_resp = BlobCopyResponse::new("acc1", "acc2")
        .with_copied(BTreeMap::from([(Id::from("b1"), Id::from("b1_new"))]))
        .with_not_copied(BTreeMap::from([(
            Id::from("b2"),
            SetError::new("notFound"),
        )]));
    let bcopy_resp_bytes = serde_json::to_vec(&bcopy_resp).expect("BlobCopyResponse serializes");
    let bcopy_resp_parsed: BlobCopyResponse =
        serde_json::from_slice(&bcopy_resp_bytes).expect("BlobCopyResponse parses");
    assert_eq!(bcopy_resp_parsed, bcopy_resp);

    let up_resp = UploadResponse::new("acc1", "blob1", 4096).with_content_type("text/plain");
    let up_resp_bytes = serde_json::to_vec(&up_resp).expect("UploadResponse serializes");
    let up_resp_parsed: UploadResponse =
        serde_json::from_slice(&up_resp_bytes).expect("UploadResponse parses");
    assert_eq!(up_resp_parsed, up_resp);

    let echo = Echo::new(json!({"ping": "pong"}));
    let echo_bytes = serde_json::to_vec(&echo).expect("Echo serializes");
    let echo_parsed: Echo<Value> = serde_json::from_slice(&echo_bytes).expect("Echo parses");
    assert_eq!(echo_parsed, echo);

    let req = Request::new(["urn:ietf:params:jmap:core"])
        .call("Foo/get", &get_req, "c1")
        .expect("call serializes")
        .with_created_ids(BTreeMap::from([(Id::from("k1"), Id::from("v1"))]));
    let req_bytes = serde_json::to_vec(&req).expect("Request serializes");
    let req_parsed: Request = serde_json::from_slice(&req_bytes).expect("Request parses");
    assert_eq!(req_parsed, req);

    let inv_err = Invocation::from_value(
        "error",
        serde_json::to_value(
            MethodError::new("invalidArguments")
                .with_description("bad args")
                .with_extra(BTreeMap::from([("arg".into(), json!("foo"))])),
        )
        .unwrap(),
        "c2",
    );
    assert!(inv_err.is_error());
    let resp = Response::new(State::from("st1"))
        .with_method_responses(vec![
            Invocation::new("Foo/get", &get_resp, "c1").expect("Invocation::new"),
        ])
        .with_method_response(inv_err)
        .with_created_ids(BTreeMap::from([(Id::from("k1"), Id::from("v1"))]));
    let resp_bytes = serde_json::to_vec(&resp).expect("Response serializes");
    let resp_parsed: Response = serde_json::from_slice(&resp_bytes).expect("Response parses");
    assert_eq!(resp_parsed, resp);

    let req_err = RequestError::new("urn:ietf:params:jmap:error:limit")
        .with_status(400)
        .with_detail("too large")
        .with_extra(BTreeMap::from([("limit".into(), json!("maxSizeRequest"))]));
    let req_err_bytes = serde_json::to_vec(&req_err).expect("RequestError serializes");
    let req_err_parsed: RequestError =
        serde_json::from_slice(&req_err_bytes).expect("RequestError parses");
    assert_eq!(req_err_parsed, req_err);
}

#[test]
fn changes_domain_filters_and_push_smoke() {
    use jmap_proto::calendars::{CalendarEventNotificationQueryFilter, CalendarEventQueryFilter};
    use jmap_proto::contacts::ContactCardQueryFilter;
    use jmap_proto::date::{civil_from_days, days_from_civil, days_in_month, is_leap_year};
    use jmap_proto::filenode::FileNodeQueryFilter;
    use jmap_proto::id::{Id, split_shared_identity};
    use jmap_proto::mail::{EmailQueryFilter, EmailSubmissionQueryFilter};
    use jmap_proto::metadata::{MetadataFilterCondition, MetadataTextFilter};
    use jmap_proto::methods::{Comparator, Filter, QueryChangesRequest};
    use jmap_proto::principals::{PrincipalQueryFilter, ShareNotificationQueryFilter};
    use jmap_proto::push::{PushSubscription, PushSubscriptionKeys, PushVerification, StateChange};
    use jmap_proto::quota::QuotaQueryFilter;
    use jmap_proto::sieve::SieveScriptQueryFilter;
    use jmap_proto::state::{State, UtcDate};
    use jmap_proto::tasks::TaskQueryFilter;
    use serde_json::json;
    use std::collections::BTreeMap;

    let email_f = EmailQueryFilter::in_mailbox("mb1")
        .in_mailbox_other_than(["mb2"])
        .min_size(100)
        .max_size(10_000)
        .has_keyword("$seen")
        .not_keyword("$draft")
        .all_in_thread_have_keyword("$flagged")
        .some_in_thread_have_keyword("$answered")
        .none_in_thread_have_keyword("$junk")
        .has_attachment(true)
        .text("hello")
        .body("world")
        .from("alice@example.com")
        .to("bob@example.com")
        .cc("carol@example.com")
        .bcc("dave@example.com")
        .subject("Greetings")
        .header("X-Custom", "val")
        .time_range(
            Some(UtcDate::new("2026-01-01T00:00:00Z")),
            Some(UtcDate::new("2026-12-31T23:59:59Z")),
        )
        .after("2026-01-01T00:00:00Z")
        .before("2026-12-31T23:59:59Z")
        .with_has_smime(true)
        .with_has_verified_smime(false);
    let email_qcr = QueryChangesRequest::new("acc1", "qs1")
        .filter(Filter::and([
            Filter::condition(email_f.clone()),
            Filter::not([Filter::from(email_f)]),
        ]))
        .sort([Comparator::descending("receivedAt").with_collation("i;ascii-casemap")])
        .max_changes(50)
        .up_to_id("m99")
        .calculate_total()
        .collapse_threads();
    let email_qcr_bytes = serde_json::to_vec(&email_qcr).expect("email qcr serializes");
    let email_qcr_parsed: QueryChangesRequest<Filter<EmailQueryFilter>> =
        serde_json::from_slice(&email_qcr_bytes).expect("email qcr parses");
    assert_eq!(email_qcr_parsed, email_qcr);

    let sub_f = EmailSubmissionQueryFilter::new()
        .with_identity_ids(["id1"])
        .with_email_ids(["m1"])
        .with_thread_ids(["t1"])
        .with_undo_status("pending")
        .time_range(
            Some(UtcDate::new("2026-01-01T00:00:00Z")),
            Some(UtcDate::new("2026-02-01T00:00:00Z")),
        )
        .after("2026-01-01T00:00:00Z")
        .before("2026-02-01T00:00:00Z");
    let sub_qcr = QueryChangesRequest::new("acc1", "qs2").filter(Filter::condition(sub_f));
    let sub_qcr_bytes = serde_json::to_vec(&sub_qcr).expect("sub qcr serializes");
    let sub_qcr_parsed: QueryChangesRequest<Filter<EmailSubmissionQueryFilter>> =
        serde_json::from_slice(&sub_qcr_bytes).expect("sub qcr parses");
    assert_eq!(sub_qcr_parsed, sub_qcr);

    let card_f = ContactCardQueryFilter::in_address_book("ab1")
        .uid("u1")
        .has_member("m1")
        .created_before("2026-06-01T00:00:00Z")
        .created_after("2026-01-01T00:00:00Z")
        .updated_before("2026-07-01T00:00:00Z")
        .updated_after("2026-02-01T00:00:00Z")
        .text("Alice")
        .name("Alice Smith")
        .name_given("Alice")
        .name_surname("Smith")
        .name_surname2("Jones")
        .nickname("Ally")
        .organization("Example Corp")
        .email("alice@example.com")
        .phone("+123456789")
        .online_service("matrix")
        .address("Main St")
        .note("vip")
        .kind("individual");
    let card_qcr = QueryChangesRequest::new("acc1", "qs3").filter(Filter::condition(card_f));
    let card_qcr_bytes = serde_json::to_vec(&card_qcr).expect("card qcr serializes");
    let card_qcr_parsed: QueryChangesRequest<Filter<ContactCardQueryFilter>> =
        serde_json::from_slice(&card_qcr_bytes).expect("card qcr parses");
    assert_eq!(card_qcr_parsed, card_qcr);

    let cal_f = CalendarEventQueryFilter::in_calendar("cal1")
        .with_in_calendar_list(["cal1", "cal2"])
        .with_time_range("2026-01-01T00:00:00Z", "2026-12-31T23:59:59Z")
        .after("2026-01-01T00:00:00Z")
        .before("2026-12-31T23:59:59Z")
        .text("meeting")
        .title("Sync")
        .description("Weekly sync")
        .location("Room 1")
        .uid("ev-1")
        .excluded_from_calendar("cal3")
        .with_excluded_from_calendar("cal3")
        .time_zone("Europe/Berlin")
        .with_time_zone("Europe/Berlin")
        .participation_status("accepted");
    let cal_qcr = QueryChangesRequest::new("acc1", "qs4").filter(Filter::condition(cal_f));
    let cal_qcr_bytes = serde_json::to_vec(&cal_qcr).expect("cal qcr serializes");
    let cal_qcr_parsed: QueryChangesRequest<Filter<CalendarEventQueryFilter>> =
        serde_json::from_slice(&cal_qcr_bytes).expect("cal qcr parses");
    assert_eq!(cal_qcr_parsed, cal_qcr);

    let cen_f = CalendarEventNotificationQueryFilter::new()
        .with_after("2026-01-01T00:00:00Z")
        .with_before("2026-02-01T00:00:00Z")
        .with_types(["created", "updated"]);
    let cen_qcr = QueryChangesRequest::new("acc1", "qs5").filter(Filter::condition(cen_f));
    let cen_qcr_bytes = serde_json::to_vec(&cen_qcr).expect("cen qcr serializes");
    let cen_qcr_parsed: QueryChangesRequest<Filter<CalendarEventNotificationQueryFilter>> =
        serde_json::from_slice(&cen_qcr_bytes).expect("cen qcr parses");
    assert_eq!(cen_qcr_parsed, cen_qcr);

    let task_f = TaskQueryFilter::new()
        .with_task_list_id("tl1")
        .with_text("todo")
        .with_status("needs-action")
        .with_due_before("2026-05-01T00:00:00")
        .with_has_recurrence(false)
        .with_extra(BTreeMap::new());
    let task_qcr = QueryChangesRequest::new("acc1", "qs6").filter(Filter::condition(task_f));
    let task_qcr_bytes = serde_json::to_vec(&task_qcr).expect("task qcr serializes");
    let task_qcr_parsed: QueryChangesRequest<Filter<TaskQueryFilter>> =
        serde_json::from_slice(&task_qcr_bytes).expect("task qcr parses");
    assert_eq!(task_qcr_parsed, task_qcr);

    let pr_f = PrincipalQueryFilter::email("alice@example.com")
        .account_ids(["acc1"])
        .name("Alice")
        .text("Alice")
        .principal_type("individual")
        .time_zone("UTC")
        .with_extra(BTreeMap::new());
    let pr_qcr = QueryChangesRequest::new("acc1", "qs7").filter(Filter::condition(pr_f));
    let pr_qcr_bytes = serde_json::to_vec(&pr_qcr).expect("pr qcr serializes");
    let pr_qcr_parsed: QueryChangesRequest<Filter<PrincipalQueryFilter>> =
        serde_json::from_slice(&pr_qcr_bytes).expect("pr qcr parses");
    assert_eq!(pr_qcr_parsed, pr_qcr);

    let sn_f = ShareNotificationQueryFilter::new()
        .with_after("2026-01-01T00:00:00Z")
        .with_before("2026-02-01T00:00:00Z")
        .with_object_type("Calendar")
        .with_object_account_id("acc1")
        .with_extra(BTreeMap::new());
    let sn_qcr = QueryChangesRequest::new("acc1", "qs8").filter(Filter::condition(sn_f));
    let sn_qcr_bytes = serde_json::to_vec(&sn_qcr).expect("sn qcr serializes");
    let sn_qcr_parsed: QueryChangesRequest<Filter<ShareNotificationQueryFilter>> =
        serde_json::from_slice(&sn_qcr_bytes).expect("sn qcr parses");
    assert_eq!(sn_qcr_parsed, sn_qcr);

    let sieve_f = SieveScriptQueryFilter::new()
        .with_name("default")
        .with_is_active(true)
        .with_extra(BTreeMap::new());
    let sieve_qcr = QueryChangesRequest::new("acc1", "qs9").filter(Filter::condition(sieve_f));
    let sieve_qcr_bytes = serde_json::to_vec(&sieve_qcr).expect("sieve qcr serializes");
    let sieve_qcr_parsed: QueryChangesRequest<Filter<SieveScriptQueryFilter>> =
        serde_json::from_slice(&sieve_qcr_bytes).expect("sieve qcr parses");
    assert_eq!(sieve_qcr_parsed, sieve_qcr);

    let quota_f = QuotaQueryFilter::new()
        .with_name("Mail")
        .with_resource_type("octets")
        .with_scope("account")
        .with_type("Email")
        .with_extra(BTreeMap::new());
    let quota_qcr = QueryChangesRequest::new("acc1", "qs10").filter(Filter::condition(quota_f));
    let quota_qcr_bytes = serde_json::to_vec(&quota_qcr).expect("quota qcr serializes");
    let quota_qcr_parsed: QueryChangesRequest<Filter<QuotaQueryFilter>> =
        serde_json::from_slice(&quota_qcr_bytes).expect("quota qcr parses");
    assert_eq!(quota_qcr_parsed, quota_qcr);

    let fn_f = FileNodeQueryFilter::new()
        .with_parent_id("root")
        .with_descendant_id("sub")
        .with_has_parent_id(true)
        .with_name("doc.txt")
        .with_node_type("file")
        .with_role("documents")
        .with_is_executable(false)
        .with_has_blob(true)
        .with_extra(BTreeMap::new());
    let fn_qcr = QueryChangesRequest::new("acc1", "qs11").filter(Filter::condition(fn_f));
    let fn_qcr_bytes = serde_json::to_vec(&fn_qcr).expect("fn qcr serializes");
    let fn_qcr_parsed: QueryChangesRequest<Filter<FileNodeQueryFilter>> =
        serde_json::from_slice(&fn_qcr_bytes).expect("fn qcr parses");
    assert_eq!(fn_qcr_parsed, fn_qcr);

    let _ = MetadataTextFilter::new("/shared/comment", "hello").with_extra(BTreeMap::new());
    let meta_f = MetadataFilterCondition::new()
        .with_metadata_exists("/shared/vendor")
        .with_metadata_text_contains("/shared/comment", "foo")
        .with_metadata_text_equals("/shared/tag", "bar")
        .with_private_metadata_exists("/private/flag")
        .with_private_metadata_text_contains("/private/note", "secret")
        .with_private_metadata_text_equals("/private/key", "val")
        .with_extra(BTreeMap::new());
    let meta_qcr = QueryChangesRequest::new("acc1", "qs12").filter(Filter::condition(meta_f));
    let meta_qcr_bytes = serde_json::to_vec(&meta_qcr).expect("meta qcr serializes");
    let meta_qcr_parsed: QueryChangesRequest<Filter<MetadataFilterCondition>> =
        serde_json::from_slice(&meta_qcr_bytes).expect("meta qcr parses");
    assert_eq!(meta_qcr_parsed, meta_qcr);

    let sc = StateChange::new(BTreeMap::new())
        .with_changed(BTreeMap::new())
        .with_account_change("acc1", "Email", "s10")
        .with_extra(BTreeMap::from([("vendor".into(), json!(true))]));
    let sc_bytes = serde_json::to_vec(&sc).expect("StateChange serializes");
    let sc_parsed: StateChange = serde_json::from_slice(&sc_bytes).expect("StateChange parses");
    assert_eq!(sc_parsed, sc);

    let _ = PushSubscriptionKeys::new("p256", "auth")
        .with_p256dh("p256_2")
        .with_auth("auth_2");
    let ps = PushSubscription::new("dev1", "https://push.example.com")
        .with_id("ps1")
        .with_device_client_id("dev1")
        .with_url("https://push.example.com")
        .with_keys("p256", "auth")
        .with_expires("2026-12-31T23:59:59Z")
        .with_types(["Email", "CalendarEvent"])
        .with_verification_code("code123")
        .with_extra(BTreeMap::new());
    let ps_bytes = serde_json::to_vec(&ps).expect("PushSubscription serializes");
    let ps_parsed: PushSubscription =
        serde_json::from_slice(&ps_bytes).expect("PushSubscription parses");
    assert_eq!(ps_parsed, ps);

    let pv = PushVerification::new("ps1", "code123")
        .with_push_subscription_id("ps1")
        .with_verification_code("code123")
        .with_extra(BTreeMap::new());
    let pv_bytes = serde_json::to_vec(&pv).expect("PushVerification serializes");
    let pv_parsed: PushVerification =
        serde_json::from_slice(&pv_bytes).expect("PushVerification parses");
    assert_eq!(pv_parsed, pv);

    let id = Id::new("acc1/col1");
    assert!(!id.is_empty());
    assert_eq!(id.as_str(), "acc1/col1");
    assert_eq!(split_shared_identity(id.as_ref()), Some(("acc1", "col1")));

    let st = State::new("st1");
    assert_eq!(st.as_str(), "st1");
    assert_eq!(st.to_string(), "st1");

    let dt = UtcDate::new("2026-11-09T00:00:00Z");
    assert_eq!(dt.as_str(), "2026-11-09T00:00:00Z");
    assert_eq!(dt.to_string(), "2026-11-09T00:00:00Z");

    let days = days_from_civil(2024, 2, 29);
    assert_eq!(civil_from_days(days), Some((2024, 2, 29)));
    assert!(is_leap_year(2024));
    assert_eq!(days_in_month(2024, 2), 29);
}
