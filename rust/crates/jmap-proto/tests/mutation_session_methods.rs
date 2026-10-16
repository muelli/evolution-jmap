// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::id::Id;
use jmap_proto::methods::{
    AddedItem, BlobCopyRequest, BlobCopyResponse, ChangesRequest, ChangesResponse, Comparator,
    CopyRequest, CopyResponse, Echo, Filter, GetRequest, GetResponse, QueryChangesRequest,
    QueryChangesResponse, QueryRequest, QueryResponse, SetRequest, SetResponse, UploadResponse,
    operator,
};
use jmap_proto::request::ResultReference;
use jmap_proto::session::{
    Account, CAPABILITY_CALENDARS, CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_QUOTA,
    CAPABILITY_REFPLUS, CAPABILITY_SUBMISSION, CoreCapability, RefPlusCapability, Session,
    WebSocketCapability,
};
use jmap_proto::state::State;
use serde_json::{Value, json};

#[test]
fn session_quota_capability_preserves_custom_properties() {
    let mut session = Session::new(
        "alice@example.com",
        "https://jmap.example.com/api",
        "https://jmap.example.com/download/{blobId}",
        "https://jmap.example.com/upload",
        "state_s1",
    );

    assert!(session.quota_capability().is_none());

    let quota_cap_json = json!({
        "maxQuotasPerAccount": 100,
        "warnThreshold": 85
    });
    session = session.with_capability(CAPABILITY_QUOTA, quota_cap_json);

    let quota_cap = session
        .quota_capability()
        .expect("quota capability should be present");
    assert_eq!(
        quota_cap.extra.get("maxQuotasPerAccount"),
        Some(&json!(100))
    );
    assert_eq!(quota_cap.extra.get("warnThreshold"), Some(&json!(85)));
}

#[test]
fn refplus_capability_builder_methods_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorFlag".to_string(), json!(true));

    let cap = RefPlusCapability::new()
        .with_json_path(true)
        .with_filter_condition(false)
        .with_set_property(true)
        .with_extra(extra.clone());

    assert_eq!(cap.json_path, Some(true));
    assert_eq!(cap.filter_condition, Some(false));
    assert_eq!(cap.set_property, Some(true));
    assert_eq!(cap.extra, extra);

    let serialized = serde_json::to_value(&cap).expect("serialize RefPlusCapability");
    assert_eq!(serialized["jsonPath"], json!(true));
    assert_eq!(serialized["filterCondition"], json!(false));
    assert_eq!(serialized["setProperty"], json!(true));
    assert_eq!(serialized["customVendorFlag"], json!(true));

    let deserialized: RefPlusCapability =
        serde_json::from_value(serialized).expect("deserialize RefPlusCapability");
    assert_eq!(deserialized, cap);

    let session = Session::new(
        "bob@example.com",
        "https://api.example.com",
        "https://download.example.com",
        "https://upload.example.com",
        "state_0",
    )
    .with_capability(
        CAPABILITY_REFPLUS,
        serde_json::to_value(&cap).expect("serialize cap value"),
    );

    let fetched = session
        .refplus_capability()
        .expect("refplus capability in session");
    assert_eq!(fetched, cap);
}

#[test]
fn account_has_capability_predicate() {
    let account = Account::new("Primary Mail Account")
        .is_personal(true)
        .with_capability(CAPABILITY_MAIL, json!({}))
        .with_capability(CAPABILITY_QUOTA, json!({"max": 50}));

    assert!(account.has_capability(CAPABILITY_MAIL));
    assert!(account.has_capability(CAPABILITY_QUOTA));
    assert!(!account.has_capability(CAPABILITY_CALENDARS));
    assert!(!account.has_capability("unknown:capability:urn"));
}

#[test]
fn account_submission_capability_typed_accessor() {
    let account_without_sub = Account::new("Read Only Account");
    assert!(account_without_sub.submission_capability().is_none());

    let mut extensions = BTreeMap::new();
    extensions.insert("FUTURERELEASE".to_string(), vec!["LIMIT=86400".to_string()]);

    let mut extra = BTreeMap::new();
    extra.insert("customProperty".to_string(), json!("customValue"));

    let sub_json = json!({
        "maxDelayedSend": 7200,
        "submissionExtensions": extensions,
        "customProperty": "customValue"
    });

    let account_with_sub =
        Account::new("Send Account").with_capability(CAPABILITY_SUBMISSION, sub_json);

    let cap = account_with_sub
        .submission_capability()
        .expect("submission capability should be present");
    assert_eq!(cap.max_delayed_send, 7200);
    assert!(cap.supports_future_release());
    assert_eq!(
        cap.submission_extensions.get("FUTURERELEASE"),
        Some(&vec!["LIMIT=86400".to_string()])
    );
    assert_eq!(cap.extra.get("customProperty"), Some(&json!("customValue")));
}

#[test]
fn session_core_limits_and_collation_algorithms() {
    let core_json = json!({
        "maxSizeUpload": 50000000,
        "maxConcurrentUpload": 8,
        "maxSizeRequest": 10000000,
        "maxConcurrentRequests": 16,
        "maxCallsInRequest": 64,
        "maxObjectsInGet": 500,
        "maxObjectsInSet": 100,
        "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
    });

    let session = Session::new(
        "carol@example.com",
        "https://api.example.com",
        "https://download.example.com",
        "https://upload.example.com",
        "state_c",
    )
    .with_event_source_url("https://events.example.com")
    .with_capability(CAPABILITY_CORE, core_json);

    assert_eq!(session.event_source_url, "https://events.example.com");
    assert_eq!(session.max_size_upload(), Some(50000000));
    assert_eq!(session.max_concurrent_upload(), Some(8));
    assert_eq!(session.max_size_request(), Some(10000000));
    assert_eq!(session.max_concurrent_requests(), Some(16));
    assert_eq!(session.max_calls_in_request(), Some(64));
    assert_eq!(session.max_objects_in_get(), Some(500));
    assert_eq!(session.max_objects_in_set(), Some(100));
    assert_eq!(
        session.collation_algorithms(),
        Some(vec![
            "i;ascii-casemap".to_string(),
            "i;unicode-casemap".to_string()
        ])
    );

    let core_cap = session.core_capability().expect("core capability");
    assert_eq!(core_cap.max_size_upload, 50000000);
    assert_eq!(core_cap.max_calls_in_request, 64);
}

#[test]
fn session_primary_account_resolution_rules() {
    let mut session = Session::new(
        "dave@example.com",
        "https://api.example.com",
        "https://download.example.com",
        "https://upload.example.com",
        "state_d",
    )
    .with_capability(CAPABILITY_MAIL, json!({}))
    .with_capability(CAPABILITY_CALENDARS, json!({}));

    // Account 1: personal mail
    let acc1 = Account::new("Personal Account")
        .is_personal(true)
        .with_capability(CAPABILITY_MAIL, json!({}));
    // Account 2: shared calendars
    let acc2 = Account::new("Shared Calendar")
        .is_personal(false)
        .with_capability(CAPABILITY_CALENDARS, json!({}));

    session = session
        .with_account(Id::from("acc_1"), acc1)
        .with_account(Id::from("acc_2"), acc2);

    // Sole personal account resolution for mail
    assert_eq!(
        session.resolve_primary_account(CAPABILITY_MAIL),
        Some(&Id::from("acc_1"))
    );

    // No personal account for calendars, no explicit primary: returns None
    assert_eq!(session.resolve_primary_account(CAPABILITY_CALENDARS), None);

    // Explicit primary account configuration
    session = session.with_primary_account(CAPABILITY_CALENDARS, Id::from("acc_2"));
    assert_eq!(
        session.primary_account(CAPABILITY_CALENDARS),
        Some(&Id::from("acc_2"))
    );
    assert_eq!(
        session.resolve_primary_account(CAPABILITY_CALENDARS),
        Some(&Id::from("acc_2"))
    );

    // Unknown capability returns None
    assert_eq!(session.resolve_primary_account("urn:unknown"), None);
}

#[test]
fn core_capability_builders_and_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("customSetting".to_string(), json!(true));

    let core = CoreCapability::new()
        .with_max_size_upload(1024)
        .with_max_concurrent_upload(2)
        .with_max_size_request(2048)
        .with_max_concurrent_requests(4)
        .with_max_calls_in_request(8)
        .with_max_objects_in_get(16)
        .with_max_objects_in_set(32)
        .with_collation_algorithms(["i;ascii-numeric"])
        .with_extra(extra.clone());

    assert_eq!(core.max_size_upload, 1024);
    assert_eq!(core.max_concurrent_upload, 2);
    assert_eq!(core.max_size_request, 2048);
    assert_eq!(core.max_concurrent_requests, 4);
    assert_eq!(core.max_calls_in_request, 8);
    assert_eq!(core.max_objects_in_get, 16);
    assert_eq!(core.max_objects_in_set, 32);
    assert_eq!(core.collation_algorithms, vec!["i;ascii-numeric"]);
    assert_eq!(core.extra, extra);

    let val = serde_json::to_value(&core).expect("serialize CoreCapability");
    let restored: CoreCapability = serde_json::from_value(val).expect("deserialize CoreCapability");
    assert_eq!(restored, core);
}

#[test]
fn websocket_capability_builders_and_roundtrip() {
    let mut extra = BTreeMap::new();
    extra.insert("retryInterval".to_string(), json!(5000));

    let ws = WebSocketCapability::new("wss://jmap.example.com/ws")
        .supports_push(true)
        .with_extra(extra.clone());

    assert_eq!(ws.url, "wss://jmap.example.com/ws");
    assert!(ws.supports_push);
    assert_eq!(ws.extra, extra);

    let val = serde_json::to_value(&ws).expect("serialize WebSocketCapability");
    assert_eq!(val["url"], json!("wss://jmap.example.com/ws"));
    assert_eq!(val["supportsPush"], json!(true));
    assert_eq!(val["retryInterval"], json!(5000));

    let restored: WebSocketCapability =
        serde_json::from_value(val).expect("deserialize WebSocketCapability");
    assert_eq!(restored, ws);
}

#[test]
fn account_builders_and_roundtrip() {
    let mut caps = BTreeMap::new();
    caps.insert(
        CAPABILITY_SUBMISSION.to_string(),
        json!({"maxDelayedSend": 300}),
    );

    let mut extra = BTreeMap::new();
    extra.insert("quotaTier".to_string(), json!("enterprise"));

    let account = Account::new("Corporate User")
        .is_personal(true)
        .is_read_only(false)
        .with_capabilities(caps.clone())
        .with_extra(extra.clone());

    assert_eq!(account.name, "Corporate User");
    assert!(account.is_personal);
    assert!(!account.is_read_only);
    assert_eq!(account.account_capabilities, caps);
    assert_eq!(account.extra, extra);
    assert_eq!(account.max_delayed_send(), Some(300));

    let val = serde_json::to_value(&account).expect("serialize Account");
    let restored: Account = serde_json::from_value(val).expect("deserialize Account");
    assert_eq!(restored, account);
}

#[test]
fn get_request_constructors_and_result_reference() {
    let req_all = GetRequest::all("acc_10").properties(["id", "name"]);
    assert_eq!(req_all.account_id, Id::from("acc_10"));
    assert_eq!(req_all.ids, None);
    assert_eq!(req_all.ids_ref, None);
    assert_eq!(
        req_all.properties,
        Some(vec!["id".to_string(), "name".to_string()])
    );

    let res_ref = ResultReference::new("call_0", "Mailbox/query", "/ids");
    let req_ref = GetRequest::all("acc_10").ids_ref(res_ref.clone());
    assert_eq!(req_ref.ids_ref, Some(res_ref));

    let req_ids = GetRequest::ids("acc_10", [Id::from("id_a"), Id::from("id_b")]);
    assert_eq!(req_ids.ids, Some(vec![Id::from("id_a"), Id::from("id_b")]));
}

#[test]
fn get_response_constructors_and_not_found() {
    let resp = GetResponse::new(
        "acc_1",
        "st_get",
        vec![json!({"id": "item1"}), json!({"id": "item2"})],
    )
    .with_not_found([Id::from("missing1"), Id::from("missing2")]);

    assert_eq!(resp.account_id, Id::from("acc_1"));
    assert_eq!(resp.state, State::from("st_get"));
    assert_eq!(resp.list.len(), 2);
    assert_eq!(
        resp.not_found,
        vec![Id::from("missing1"), Id::from("missing2")]
    );
}

#[test]
fn set_request_and_response_constructors_and_builders() {
    let req = SetRequest::<Value>::new("acc_set")
        .if_in_state("st_pre")
        .create("c1", json!({"name": "Folder 1"}))
        .update("id_u1", json!({"name": "Renamed"}))
        .destroy("id_d1");

    assert_eq!(req.account_id, Id::from("acc_set"));
    assert_eq!(req.if_in_state, Some(State::from("st_pre")));
    assert_eq!(
        req.create.as_ref().unwrap().get("c1"),
        Some(&json!({"name": "Folder 1"}))
    );
    assert_eq!(
        req.update.as_ref().unwrap().get(&Id::from("id_u1")),
        Some(&json!({"name": "Renamed"}))
    );
    assert_eq!(req.destroy, Some(vec![Id::from("id_d1")]));

    let mut created = BTreeMap::new();
    created.insert("c1".to_string(), json!({"id": "server_id_1"}));

    let mut updated = BTreeMap::new();
    updated.insert(Id::from("id_u1"), Some(json!({"id": "id_u1"})));

    let resp = SetResponse::<Value>::new("acc_set", "st_post")
        .with_old_state("st_pre")
        .with_created(created.clone())
        .with_updated(updated.clone())
        .with_destroyed([Id::from("id_d1")]);

    assert_eq!(resp.old_state, Some(State::from("st_pre")));
    assert_eq!(resp.new_state, Some(State::from("st_post")));
    assert_eq!(resp.created, Some(created));
    assert_eq!(resp.updated, Some(updated));
    assert_eq!(resp.destroyed, Some(vec![Id::from("id_d1")]));
}

#[test]
fn query_request_serialization_omissions_and_position() {
    // Default position (0) and false flags must be omitted in serialization
    let default_query = QueryRequest::<Value>::new("acc_q");
    let val_default = serde_json::to_value(&default_query).expect("serialize default query");
    assert!(val_default.get("position").is_none());
    assert!(val_default.get("calculateTotal").is_none());
    assert!(val_default.get("collapseThreads").is_none());

    // Non-zero position (e.g. 25) and true flags must be serialized
    let custom_query = QueryRequest::<Value>::new("acc_q")
        .position(25)
        .calculate_total()
        .collapse_threads()
        .anchor("anc_1")
        .anchor_offset(-5)
        .limit(50);

    let val_custom = serde_json::to_value(&custom_query).expect("serialize custom query");
    assert_eq!(val_custom["position"], json!(25));
    assert_eq!(val_custom["calculateTotal"], json!(true));
    assert_eq!(val_custom["collapseThreads"], json!(true));
    assert_eq!(val_custom["anchor"], json!("anc_1"));
    assert_eq!(val_custom["anchorOffset"], json!(-5));
    assert_eq!(val_custom["limit"], json!(50));
}

#[test]
fn comparator_constructors_and_default_is_ascending() {
    let asc = Comparator::ascending("receivedAt").with_collation("i;unicode-casemap");
    assert_eq!(asc.property, "receivedAt");
    assert!(asc.is_ascending);
    assert_eq!(asc.collation, Some("i;unicode-casemap".to_string()));

    let desc = Comparator::descending("size");
    assert_eq!(desc.property, "size");
    assert!(!desc.is_ascending);
    assert_eq!(desc.collation, None);

    // When deserializing json omitting isAscending, it defaults to true
    let json_omitted = json!({"property": "date"});
    let comp: Comparator = serde_json::from_value(json_omitted).expect("deserialize comparator");
    assert!(comp.is_ascending);
}

#[test]
fn filter_operators_and_composition() {
    let cond1 = json!({"hasAttachment": true});
    let cond2 = json!({"inMailbox": "mbx_inbox"});

    let filter_and = Filter::and([
        Filter::condition(cond1.clone()),
        Filter::condition(cond2.clone()),
    ]);
    let filter_or = Filter::or([
        Filter::condition(cond1.clone()),
        Filter::condition(cond2.clone()),
    ]);
    let filter_not = Filter::not([Filter::from(cond1.clone())]);

    let val_and = serde_json::to_value(&filter_and).expect("serialize and filter");
    assert_eq!(val_and["operator"], json!(operator::AND));
    assert_eq!(val_and["conditions"].as_array().unwrap().len(), 2);

    let val_or = serde_json::to_value(&filter_or).expect("serialize or filter");
    assert_eq!(val_or["operator"], json!(operator::OR));

    let val_not = serde_json::to_value(&filter_not).expect("serialize not filter");
    assert_eq!(val_not["operator"], json!(operator::NOT));
}

#[test]
fn query_response_builders_and_changes() {
    let qresp = QueryResponse::new("acc_1", "qst_1", [Id::from("item_a"), Id::from("item_b")])
        .with_total(100)
        .with_limit(50)
        .with_position(10)
        .can_calculate_changes(true);

    assert_eq!(qresp.account_id, Id::from("acc_1"));
    assert_eq!(qresp.query_state, State::from("qst_1"));
    assert_eq!(qresp.total, Some(100));
    assert_eq!(qresp.limit, Some(50));
    assert_eq!(qresp.position, 10);
    assert!(qresp.can_calculate_changes);
    assert_eq!(qresp.ids, vec![Id::from("item_a"), Id::from("item_b")]);
}

#[test]
fn changes_and_query_changes_requests_and_responses() {
    let chg_req = ChangesRequest::new("acc_1", "st_0").max_changes(25);
    assert_eq!(chg_req.account_id, Id::from("acc_1"));
    assert_eq!(chg_req.since_state, State::from("st_0"));
    assert_eq!(chg_req.max_changes, Some(25));

    let chg_resp = ChangesResponse::new("acc_1", "st_0", "st_1")
        .with_created([Id::from("new_1")])
        .with_updated([Id::from("up_1")])
        .with_destroyed([Id::from("del_1")])
        .has_more_changes(true);

    assert_eq!(chg_resp.created, vec![Id::from("new_1")]);
    assert_eq!(chg_resp.updated, vec![Id::from("up_1")]);
    assert_eq!(chg_resp.destroyed, vec![Id::from("del_1")]);
    assert!(chg_resp.has_more_changes);

    let qc_req = QueryChangesRequest::<Value>::new("acc_1", "qst_0")
        .max_changes(10)
        .up_to_id("up_to_99")
        .calculate_total()
        .collapse_threads();

    assert_eq!(qc_req.since_query_state, State::from("qst_0"));
    assert_eq!(qc_req.max_changes, Some(10));
    assert_eq!(qc_req.up_to_id, Some(Id::from("up_to_99")));
    assert!(qc_req.calculate_total);
    assert!(qc_req.collapse_threads);

    let qc_resp = QueryChangesResponse::new("acc_1", "qst_0", "qst_1")
        .with_added([AddedItem::new("item_10", 0)])
        .with_removed([Id::from("item_5")])
        .with_total(42);

    assert_eq!(qc_resp.added, vec![AddedItem::new("item_10", 0)]);
    assert_eq!(qc_resp.removed, vec![Id::from("item_5")]);
    assert_eq!(qc_resp.total, Some(42));
}

#[test]
fn copy_and_blob_copy_request_response_types() {
    let copy_req = CopyRequest::<Value>::new("acc_from", "acc_to")
        .if_from_in_state("st_from_0")
        .if_in_state("st_to_0")
        .copy_object("cp_1", json!({"subject": "copied"}))
        .on_success_destroy_original()
        .destroy_from_if_in_state("st_from_0");

    assert_eq!(copy_req.from_account_id, Id::from("acc_from"));
    assert_eq!(copy_req.account_id, Id::from("acc_to"));
    assert_eq!(copy_req.if_from_in_state, Some(State::from("st_from_0")));
    assert_eq!(copy_req.if_in_state, Some(State::from("st_to_0")));
    assert!(copy_req.on_success_destroy_original);
    assert_eq!(
        copy_req.destroy_from_if_in_state,
        Some(State::from("st_from_0"))
    );

    let mut created = BTreeMap::new();
    created.insert("cp_1".to_string(), json!({"id": "target_id_1"}));

    let copy_resp = CopyResponse::<Value>::new("acc_from", "acc_to", "st_to_1")
        .with_old_state("st_to_0")
        .with_created(created.clone());

    assert_eq!(copy_resp.old_state, Some(State::from("st_to_0")));
    assert_eq!(copy_resp.new_state, State::from("st_to_1"));
    assert_eq!(copy_resp.created, Some(created));

    let blob_req = BlobCopyRequest::new(
        "acc_from",
        "acc_to",
        [Id::from("blob_a"), Id::from("blob_b")],
    );
    assert_eq!(blob_req.from_account_id, Id::from("acc_from"));
    assert_eq!(blob_req.account_id, Id::from("acc_to"));
    assert_eq!(
        blob_req.blob_ids,
        vec![Id::from("blob_a"), Id::from("blob_b")]
    );

    let mut copied = BTreeMap::new();
    copied.insert(Id::from("blob_a"), Id::from("blob_target_a"));

    let blob_resp = BlobCopyResponse::new("acc_from", "acc_to").with_copied(copied.clone());
    assert_eq!(blob_resp.copied, Some(copied));
    assert_eq!(blob_resp.not_copied, None);
}

#[test]
fn upload_response_and_echo_types() {
    let upload = UploadResponse::new("acc_u", "blob_123", 4096).with_content_type("text/plain");

    assert_eq!(upload.account_id, Id::from("acc_u"));
    assert_eq!(upload.blob_id, Id::from("blob_123"));
    assert_eq!(upload.size, 4096);
    assert_eq!(upload.content_type, Some("text/plain".to_string()));

    let echo = Echo::new(json!({"ping": "pong"}));
    assert_eq!(echo.payload, json!({"ping": "pong"}));
}
