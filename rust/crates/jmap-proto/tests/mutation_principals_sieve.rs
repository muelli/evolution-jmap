// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::calendars::CalendarEvent;
use jmap_proto::error::SetError;
use jmap_proto::id::Id;
use jmap_proto::methods::SetRequest;
use jmap_proto::principals::{
    BusyPeriod, GetAvailabilityRequest, GetAvailabilityResponse, Principal,
    PrincipalAvailabilityCapability, PrincipalQueryFilter, PrincipalsCapability,
    PrincipalsOwnerCapability, ShareNotification, ShareNotificationQueryFilter, busy_status,
    principal_set_error, principal_type, share_notification_object_type,
};
use jmap_proto::sieve::{
    CAPABILITY_SIEVE, SieveCapability, SieveScript, SieveScriptQueryFilter, SieveScriptSetRequest,
    SieveScriptValidateRequest, SieveScriptValidateResponse, sieve_set_error,
};
use jmap_proto::state::UtcDate;
use serde_json::json;

#[test]
fn principal_builders_and_properties() {
    let mut capabilities = BTreeMap::new();
    capabilities.insert(
        "urn:ietf:params:jmap:calendars".to_string(),
        json!({"mayGetAvailability": true}),
    );

    let mut send_to = BTreeMap::new();
    send_to.insert("imip".to_string(), "mailto:alice@example.com".to_string());

    let mut extra = BTreeMap::new();
    extra.insert("customVendorKey".to_string(), json!("customVendorValue"));
    extra.insert("quotaTier".to_string(), json!(5));

    let principal = Principal::new("Alice Example")
        .with_id("P1")
        .with_type(principal_type::INDIVIDUAL)
        .with_email("alice@example.com")
        .with_description("Primary engineering account")
        .with_time_zone("Europe/Berlin")
        .with_secret("sec-token-xyz")
        .with_send_to(send_to.clone())
        .with_capabilities(capabilities.clone())
        .is_personal(true)
        .with_extra(extra);

    assert_eq!(principal.name, "Alice Example");
    assert_eq!(principal.id, Some(Id::from("P1")));
    assert_eq!(
        principal.principal_type,
        Some(principal_type::INDIVIDUAL.to_string())
    );
    assert_eq!(principal.email, Some("alice@example.com".to_string()));
    assert_eq!(
        principal.description,
        Some("Primary engineering account".to_string())
    );
    assert_eq!(principal.time_zone, Some("Europe/Berlin".to_string()));
    assert_eq!(principal.secret, Some("sec-token-xyz".to_string()));
    assert_eq!(principal.send_to, Some(send_to));
    assert_eq!(principal.capabilities, capabilities);
    assert_eq!(principal.is_personal, Some(true));
    assert_eq!(
        principal.extra.get("customVendorKey"),
        Some(&json!("customVendorValue"))
    );
    assert_eq!(principal.extra.get("quotaTier"), Some(&json!(5)));

    let val = serde_json::to_value(&principal).expect("serialize Principal");
    assert_eq!(val["name"], "Alice Example");
    assert_eq!(val["id"], "P1");
    assert_eq!(val["type"], "individual");
    assert_eq!(val["email"], "alice@example.com");
    assert_eq!(val["description"], "Primary engineering account");
    assert_eq!(val["timeZone"], "Europe/Berlin");
    assert_eq!(val["secret"], "sec-token-xyz");
    assert_eq!(val["sendTo"]["imip"], "mailto:alice@example.com");
    assert_eq!(
        val["capabilities"]["urn:ietf:params:jmap:calendars"]["mayGetAvailability"],
        true
    );
    assert_eq!(val["isPersonal"], true);
    assert_eq!(val["customVendorKey"], "customVendorValue");
    assert_eq!(val["quotaTier"], 5);

    let roundtripped: Principal = serde_json::from_value(val).expect("deserialize Principal");
    assert_eq!(roundtripped, principal);
}

#[test]
fn principal_query_filter_builders_and_properties() {
    let filter = PrincipalQueryFilter::email("alice@example.com")
        .name("Alice Example")
        .text("Alice");

    assert_eq!(filter.email, Some("alice@example.com".to_string()));
    assert_eq!(filter.name, Some("Alice Example".to_string()));
    assert_eq!(filter.text, Some("Alice".to_string()));

    let val = serde_json::to_value(&filter).expect("serialize PrincipalQueryFilter");
    assert_eq!(val["email"], "alice@example.com");
    assert_eq!(val["name"], "Alice Example");
    assert_eq!(val["text"], "Alice");

    let roundtripped: PrincipalQueryFilter =
        serde_json::from_value(val).expect("deserialize PrincipalQueryFilter");
    assert_eq!(roundtripped, filter);
}

#[test]
fn get_availability_request_and_response() {
    let request = GetAvailabilityRequest::new(
        "A1",
        "P1",
        UtcDate::new("2026-10-01T08:00:00Z"),
        UtcDate::new("2026-10-01T17:00:00Z"),
    )
    .show_details();

    assert_eq!(request.account_id, Id::from("A1"));
    assert_eq!(request.id, Id::from("P1"));
    assert_eq!(request.utc_start, UtcDate::new("2026-10-01T08:00:00Z"));
    assert_eq!(request.utc_end, UtcDate::new("2026-10-01T17:00:00Z"));
    assert!(request.show_details);
    assert!(request.event_properties.is_none());

    let val = serde_json::to_value(&request).expect("serialize GetAvailabilityRequest");
    assert_eq!(val["accountId"], "A1");
    assert_eq!(val["id"], "P1");
    assert_eq!(val["utcStart"], "2026-10-01T08:00:00Z");
    assert_eq!(val["utcEnd"], "2026-10-01T17:00:00Z");
    assert_eq!(val["showDetails"], true);

    let roundtripped: GetAvailabilityRequest =
        serde_json::from_value(val).expect("deserialize GetAvailabilityRequest");
    assert_eq!(roundtripped, request);

    let event = CalendarEvent::new().with_title("Sprint Review");

    let period = BusyPeriod::new(
        UtcDate::new("2026-10-01T10:00:00Z"),
        UtcDate::new("2026-10-01T11:00:00Z"),
        busy_status::CONFIRMED,
    )
    .with_event(event);

    assert_eq!(period.utc_start, UtcDate::new("2026-10-01T10:00:00Z"));
    assert_eq!(period.utc_end, UtcDate::new("2026-10-01T11:00:00Z"));
    assert_eq!(period.busy_status, busy_status::CONFIRMED);
    assert_eq!(
        period.event.as_ref().and_then(|e| e.title.as_deref()),
        Some("Sprint Review")
    );

    let response = GetAvailabilityResponse::new(vec![period.clone()]);
    assert_eq!(response.list.len(), 1);
    assert_eq!(response.list[0], period);

    let val_resp = serde_json::to_value(&response).expect("serialize GetAvailabilityResponse");
    assert_eq!(val_resp["list"][0]["busyStatus"], "confirmed");
    assert_eq!(val_resp["list"][0]["event"]["title"], "Sprint Review");

    let roundtripped_resp: GetAvailabilityResponse =
        serde_json::from_value(val_resp).expect("deserialize GetAvailabilityResponse");
    assert_eq!(roundtripped_resp, response);
}

#[test]
fn share_notification_builders_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customAuditSeq".to_string(), json!(9012));
    extra.insert("notifiedByCluster".to_string(), json!("eu-central-1"));

    let changer = Principal::new("Bob Admin")
        .with_id("P2")
        .with_email("bob@example.com");

    let notification = ShareNotification::new(
        UtcDate::new("2026-10-15T12:00:00Z"),
        share_notification_object_type::CALENDAR,
        "C1",
        "A1",
    )
    .with_id("SN10")
    .with_changed_by(changer.clone())
    .with_name("Team Shared Calendar")
    .with_old_rights(json!({"mayRead": false}))
    .with_new_rights(json!({"mayRead": true, "mayWrite": true}))
    .with_extra(extra);

    assert_eq!(notification.id, Some(Id::from("SN10")));
    assert_eq!(notification.created, UtcDate::new("2026-10-15T12:00:00Z"));
    assert_eq!(notification.changed_by, Some(changer));
    assert_eq!(
        notification.object_type,
        share_notification_object_type::CALENDAR
    );
    assert_eq!(notification.object_id, Id::from("C1"));
    assert_eq!(notification.object_account_id, Id::from("A1"));
    assert_eq!(notification.name, Some("Team Shared Calendar".to_string()));
    assert_eq!(notification.old_rights, Some(json!({"mayRead": false})));
    assert_eq!(
        notification.new_rights,
        Some(json!({"mayRead": true, "mayWrite": true}))
    );
    assert_eq!(notification.extra.get("customAuditSeq"), Some(&json!(9012)));
    assert_eq!(
        notification.extra.get("notifiedByCluster"),
        Some(&json!("eu-central-1"))
    );

    let val = serde_json::to_value(&notification).expect("serialize ShareNotification");
    assert_eq!(val["id"], "SN10");
    assert_eq!(val["created"], "2026-10-15T12:00:00Z");
    assert_eq!(val["objectType"], "Calendar");
    assert_eq!(val["objectId"], "C1");
    assert_eq!(val["objectAccountId"], "A1");
    assert_eq!(val["name"], "Team Shared Calendar");
    assert_eq!(val["oldRights"]["mayRead"], false);
    assert_eq!(val["newRights"]["mayWrite"], true);
    assert_eq!(val["customAuditSeq"], 9012);
    assert_eq!(val["notifiedByCluster"], "eu-central-1");

    let roundtripped: ShareNotification =
        serde_json::from_value(val).expect("deserialize ShareNotification");
    assert_eq!(roundtripped, notification);
}

#[test]
fn share_notification_query_filter_builders_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("filterSourceScope".to_string(), json!("cluster-local"));

    let filter = ShareNotificationQueryFilter::new()
        .with_after(UtcDate::new("2026-10-01T00:00:00Z"))
        .with_before(UtcDate::new("2026-10-15T00:00:00Z"))
        .with_object_type(share_notification_object_type::MAILBOX)
        .with_extra(extra);

    assert_eq!(filter.after, Some(UtcDate::new("2026-10-01T00:00:00Z")));
    assert_eq!(filter.before, Some(UtcDate::new("2026-10-15T00:00:00Z")));
    assert_eq!(
        filter.object_type,
        Some(share_notification_object_type::MAILBOX.to_string())
    );
    assert_eq!(
        filter.extra.get("filterSourceScope"),
        Some(&json!("cluster-local"))
    );

    let val = serde_json::to_value(&filter).expect("serialize ShareNotificationQueryFilter");
    assert_eq!(val["after"], "2026-10-01T00:00:00Z");
    assert_eq!(val["before"], "2026-10-15T00:00:00Z");
    assert_eq!(val["objectType"], "Mailbox");
    assert_eq!(val["filterSourceScope"], "cluster-local");

    let roundtripped: ShareNotificationQueryFilter =
        serde_json::from_value(val).expect("deserialize ShareNotificationQueryFilter");
    assert_eq!(roundtripped, filter);
}

#[test]
fn principals_capabilities_and_constants() {
    let mut cap_extra = BTreeMap::new();
    cap_extra.insert("supportPrincipalAvatars".to_string(), json!(true));

    let cap = PrincipalsCapability::new()
        .with_max_principals_per_get(250)
        .with_extra(cap_extra);

    assert_eq!(cap.max_principals_per_get, Some(250));
    assert_eq!(cap.extra.get("supportPrincipalAvatars"), Some(&json!(true)));

    let val_cap = serde_json::to_value(&cap).expect("serialize PrincipalsCapability");
    assert_eq!(val_cap["maxPrincipalsPerGet"], 250);
    assert_eq!(val_cap["supportPrincipalAvatars"], true);

    let roundtripped_cap: PrincipalsCapability =
        serde_json::from_value(val_cap).expect("deserialize PrincipalsCapability");
    assert_eq!(roundtripped_cap, cap);

    let mut owner_extra = BTreeMap::new();
    owner_extra.insert("directorySource".to_string(), json!("ldap-synced"));

    let owner = PrincipalsOwnerCapability::new("A1", "P1")
        .with_account_id_for_principal("A2")
        .with_principal_id("P2")
        .with_extra(owner_extra);

    assert_eq!(owner.account_id_for_principal, Id::from("A2"));
    assert_eq!(owner.principal_id, Id::from("P2"));
    assert_eq!(
        owner.extra.get("directorySource"),
        Some(&json!("ldap-synced"))
    );

    let val_owner = serde_json::to_value(&owner).expect("serialize PrincipalsOwnerCapability");
    assert_eq!(val_owner["accountIdForPrincipal"], "A2");
    assert_eq!(val_owner["principalId"], "P2");
    assert_eq!(val_owner["directorySource"], "ldap-synced");

    let roundtripped_owner: PrincipalsOwnerCapability =
        serde_json::from_value(val_owner).expect("deserialize PrincipalsOwnerCapability");
    assert_eq!(roundtripped_owner, owner);

    let mut avail_extra = BTreeMap::new();
    avail_extra.insert("extendedLookahead".to_string(), json!("P365D"));

    let avail = PrincipalAvailabilityCapability::new()
        .with_max_availability_duration("P90D")
        .with_extra(avail_extra);

    assert_eq!(avail.max_availability_duration, Some("P90D".to_string()));
    assert_eq!(avail.extra.get("extendedLookahead"), Some(&json!("P365D")));

    let val_avail =
        serde_json::to_value(&avail).expect("serialize PrincipalAvailabilityCapability");
    assert_eq!(val_avail["maxAvailabilityDuration"], "P90D");
    assert_eq!(val_avail["extendedLookahead"], "P365D");

    let roundtripped_avail: PrincipalAvailabilityCapability =
        serde_json::from_value(val_avail).expect("deserialize PrincipalAvailabilityCapability");
    assert_eq!(roundtripped_avail, avail);

    // Verify well-known constants
    assert_eq!(principal_type::INDIVIDUAL, "individual");
    assert_eq!(principal_type::GROUP, "group");
    assert_eq!(principal_type::RESOURCE, "resource");
    assert_eq!(principal_type::LOCATION, "location");
    assert_eq!(principal_type::OTHER, "other");

    assert_eq!(busy_status::CONFIRMED, "confirmed");
    assert_eq!(busy_status::TENTATIVE, "tentative");
    assert_eq!(busy_status::UNAVAILABLE, "unavailable");

    assert_eq!(share_notification_object_type::ADDRESS_BOOK, "AddressBook");
    assert_eq!(share_notification_object_type::CALENDAR, "Calendar");
    assert_eq!(share_notification_object_type::MAILBOX, "Mailbox");

    assert_eq!(principal_set_error::FORBIDDEN, "forbidden");
    assert_eq!(
        principal_set_error::PRINCIPAL_ALREADY_EXISTS,
        "principalAlreadyExists"
    );
    assert_eq!(principal_set_error::INVALID_PROPERTIES, "invalidProperties");
}

#[test]
fn sieve_capability_builders_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("compilerTier".to_string(), json!("jit-optimized"));

    let cap = SieveCapability::new(65536)
        .with_max_number_scripts(20)
        .with_implementation("Pigeonhole-JMAP-0.8")
        .with_sieve_extensions(["fileinto", "reject", "envelope"])
        .with_sieve_extension("vacation")
        .with_extra(extra);

    assert_eq!(cap.max_size_script, 65536);
    assert_eq!(cap.max_number_scripts, Some(20));
    assert_eq!(cap.implementation, Some("Pigeonhole-JMAP-0.8".to_string()));
    assert_eq!(
        cap.sieve_extensions,
        vec!["fileinto", "reject", "envelope", "vacation"]
    );
    assert_eq!(cap.extra.get("compilerTier"), Some(&json!("jit-optimized")));

    let val = serde_json::to_value(&cap).expect("serialize SieveCapability");
    assert_eq!(val["maxSizeScript"], 65536);
    assert_eq!(val["maxNumberScripts"], 20);
    assert_eq!(val["implementation"], "Pigeonhole-JMAP-0.8");
    assert_eq!(
        val["sieveExtensions"],
        json!(["fileinto", "reject", "envelope", "vacation"])
    );
    assert_eq!(val["compilerTier"], "jit-optimized");

    let roundtripped: SieveCapability =
        serde_json::from_value(val).expect("deserialize SieveCapability");
    assert_eq!(roundtripped, cap);

    assert_eq!(CAPABILITY_SIEVE, "urn:ietf:params:jmap:sieve");
}

#[test]
fn sieve_script_builders_properties_and_null_name_deserialization() {
    let mut extra = BTreeMap::new();
    extra.insert("lastCompiledBytes".to_string(), json!(4096));

    let script = SieveScript::new("standard-filter", "blob-sieve-001")
        .with_id("script-01")
        .with_name("custom-filter-rules")
        .is_active(true)
        .with_extra(extra);

    assert_eq!(script.id, Some(Id::from("script-01")));
    assert_eq!(script.name, "custom-filter-rules");
    assert_eq!(script.blob_id, Id::from("blob-sieve-001"));
    assert!(script.is_active);
    assert_eq!(script.extra.get("lastCompiledBytes"), Some(&json!(4096)));

    let val = serde_json::to_value(&script).expect("serialize SieveScript");
    assert_eq!(val["id"], "script-01");
    assert_eq!(val["name"], "custom-filter-rules");
    assert_eq!(val["blobId"], "blob-sieve-001");
    assert_eq!(val["isActive"], true);
    assert_eq!(val["lastCompiledBytes"], 4096);

    let roundtripped: SieveScript = serde_json::from_value(val).expect("deserialize SieveScript");
    assert_eq!(roundtripped, script);

    // RFC 9661 null or omitted name deserializes cleanly to empty string
    let null_name_json = json!({
        "id": "script-02",
        "blobId": "blob-sieve-002",
        "name": null,
        "isActive": false
    });
    let null_name_script: SieveScript =
        serde_json::from_value(null_name_json).expect("deserialize null name SieveScript");
    assert_eq!(null_name_script.id, Some(Id::from("script-02")));
    assert_eq!(null_name_script.name, "");
    assert_eq!(null_name_script.blob_id, Id::from("blob-sieve-002"));
    assert!(!null_name_script.is_active);
}

#[test]
fn sieve_script_set_request_activating_and_deactivating() {
    let base_set = SetRequest::<SieveScript>::new("acc-01");
    let activating_req = SieveScriptSetRequest::new(base_set.clone()).activating("script-01");

    assert_eq!(activating_req.set.account_id, Id::from("acc-01"));
    assert_eq!(
        activating_req.on_success_activate_script,
        Some(json!("script-01"))
    );
    assert!(activating_req.on_success_deactivate_script.is_none());

    let val_act =
        serde_json::to_value(&activating_req).expect("serialize SieveScriptSetRequest activating");
    assert_eq!(val_act["accountId"], "acc-01");
    assert_eq!(val_act["onSuccessActivateScript"], "script-01");

    let deactivating_req = SieveScriptSetRequest::new(base_set)
        .deactivating()
        .with_deactivate_script(true);

    assert_eq!(
        deactivating_req.on_success_activate_script,
        Some(serde_json::Value::Null)
    );
    assert_eq!(deactivating_req.on_success_deactivate_script, Some(true));

    let val_deact = serde_json::to_value(&deactivating_req)
        .expect("serialize SieveScriptSetRequest deactivating");
    assert_eq!(
        val_deact["onSuccessActivateScript"],
        serde_json::Value::Null
    );
    assert_eq!(val_deact["onSuccessDeactivateScript"], true);
}

#[test]
fn sieve_script_query_filter_builders_and_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("scriptQueryScope".to_string(), json!("active-only"));

    let filter = SieveScriptQueryFilter::new()
        .with_name("spam-filter")
        .with_is_active(true)
        .with_extra(extra);

    assert_eq!(filter.name, Some("spam-filter".to_string()));
    assert_eq!(filter.is_active, Some(true));
    assert_eq!(
        filter.extra.get("scriptQueryScope"),
        Some(&json!("active-only"))
    );

    let val = serde_json::to_value(&filter).expect("serialize SieveScriptQueryFilter");
    assert_eq!(val["name"], "spam-filter");
    assert_eq!(val["isActive"], true);
    assert_eq!(val["scriptQueryScope"], "active-only");

    let roundtripped: SieveScriptQueryFilter =
        serde_json::from_value(val).expect("deserialize SieveScriptQueryFilter");
    assert_eq!(roundtripped, filter);
}

#[test]
fn sieve_script_validate_request_and_response() {
    let mut req_extra = BTreeMap::new();
    req_extra.insert("validateStrictGrammar".to_string(), json!(true));

    let req = SieveScriptValidateRequest::new("acc-01", "blob-script-50").with_extra(req_extra);

    assert_eq!(req.account_id, Id::from("acc-01"));
    assert_eq!(req.blob_id, Id::from("blob-script-50"));
    assert_eq!(req.extra.get("validateStrictGrammar"), Some(&json!(true)));

    let val_req = serde_json::to_value(&req).expect("serialize SieveScriptValidateRequest");
    assert_eq!(val_req["accountId"], "acc-01");
    assert_eq!(val_req["blobId"], "blob-script-50");
    assert_eq!(val_req["validateStrictGrammar"], true);

    let roundtripped_req: SieveScriptValidateRequest =
        serde_json::from_value(val_req).expect("deserialize SieveScriptValidateRequest");
    assert_eq!(roundtripped_req, req);

    let mut resp_extra = BTreeMap::new();
    resp_extra.insert("validatorBuild".to_string(), json!("v1.4.2"));

    let valid_resp = SieveScriptValidateResponse::valid("acc-01").with_extra(resp_extra.clone());
    assert_eq!(valid_resp.account_id, Id::from("acc-01"));
    assert!(valid_resp.error.is_none());
    assert!(valid_resp.is_valid());
    assert_eq!(
        valid_resp.extra.get("validatorBuild"),
        Some(&json!("v1.4.2"))
    );

    let val_valid =
        serde_json::to_value(&valid_resp).expect("serialize valid SieveScriptValidateResponse");
    assert_eq!(val_valid["accountId"], "acc-01");
    assert!(val_valid.get("error").is_none());
    assert_eq!(val_valid["validatorBuild"], "v1.4.2");

    let err = SetError::new(sieve_set_error::INVALID_SIEVE)
        .with_description("syntax error on line 12: unexpected token");
    let invalid_resp = SieveScriptValidateResponse::invalid("acc-01", err).with_extra(resp_extra);

    assert_eq!(invalid_resp.account_id, Id::from("acc-01"));
    assert!(!invalid_resp.is_valid());
    assert_eq!(
        invalid_resp.error.as_ref().map(|e| e.error_type.as_str()),
        Some(sieve_set_error::INVALID_SIEVE)
    );
    assert_eq!(
        invalid_resp
            .error
            .as_ref()
            .and_then(|e| e.description.as_deref()),
        Some("syntax error on line 12: unexpected token")
    );

    let val_invalid =
        serde_json::to_value(&invalid_resp).expect("serialize invalid SieveScriptValidateResponse");
    assert_eq!(val_invalid["accountId"], "acc-01");
    assert_eq!(val_invalid["error"]["type"], "invalidSieve");
    assert_eq!(
        val_invalid["error"]["description"],
        "syntax error on line 12: unexpected token"
    );

    let roundtripped_invalid: SieveScriptValidateResponse = serde_json::from_value(val_invalid)
        .expect("deserialize invalid SieveScriptValidateResponse");
    assert_eq!(roundtripped_invalid, invalid_resp);

    assert_eq!(sieve_set_error::INVALID_SIEVE, "invalidSieve");
    assert_eq!(sieve_set_error::SIEVE_IS_ACTIVE, "sieveIsActive");
}
