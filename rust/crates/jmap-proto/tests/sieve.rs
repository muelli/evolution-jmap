// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 9265 (JMAP for Sieve Scripts) unit and roundtrip tests.

use jmap_proto::session::{CAPABILITY_SIEVE, Session};
use jmap_proto::sieve::{
    SieveCapability, SieveScript, SieveScriptQueryFilter, SieveScriptValidateError,
    SieveScriptValidateRequest, SieveScriptValidateResponse, sieve_set_error,
};
use serde_json::json;

#[test]
fn sieve_script_round_trips_through_camel_case_json() {
    let script = SieveScript::new("Spam Filter", "blob_sieve_42")
        .with_id("script_1")
        .is_active(true);

    let val = serde_json::to_value(&script).expect("serialize SieveScript");
    assert_eq!(val["id"], "script_1");
    assert_eq!(val["name"], "Spam Filter");
    assert_eq!(val["blobId"], "blob_sieve_42");
    assert_eq!(val["isActive"], true);

    let back: SieveScript = serde_json::from_value(val).expect("deserialize SieveScript");
    assert_eq!(back, script);
}

#[test]
fn sieve_script_minimal_deserialization_and_forward_compatibility() {
    let raw = json!({
        "id": "script_min",
        "name": "Vacation Autoresponder",
        "blobId": "blob_vac_99",
        "futureSieveMeta": {
            "lastCompiled": "2026-08-30T10:00:00Z"
        }
    });

    let s: SieveScript = serde_json::from_value(raw).expect("deserializes minimal script");
    assert_eq!(s.id.as_ref().unwrap().as_str(), "script_min");
    assert_eq!(s.name, "Vacation Autoresponder");
    assert_eq!(s.blob_id.as_str(), "blob_vac_99");
    assert!(!s.is_active);
    assert_eq!(
        s.extra["futureSieveMeta"]["lastCompiled"],
        "2026-08-30T10:00:00Z"
    );
}

#[test]
fn sieve_capability_roundtrip_and_builders() {
    let cap = SieveCapability::new(65_536)
        .with_max_number_scripts(10)
        .with_implementation("ManageSieve 2.0")
        .with_sieve_extensions(vec!["fileinto", "reject", "vacation", "imap4flags"])
        .with_sieve_extension("vnd.custom-sieve");

    let val = serde_json::to_value(&cap).expect("serialize SieveCapability");
    assert_eq!(val["maxSizeScript"], 65_536);
    assert_eq!(val["maxNumberScripts"], 10);
    assert_eq!(val["implementation"], "ManageSieve 2.0");
    assert_eq!(
        val["sieveExtensions"],
        json!([
            "fileinto",
            "reject",
            "vacation",
            "imap4flags",
            "vnd.custom-sieve"
        ])
    );

    let back: SieveCapability = serde_json::from_value(val).expect("deserialize SieveCapability");
    assert_eq!(back, cap);
}

#[test]
fn sieve_query_filter_roundtrip_and_builders() {
    let filter = SieveScriptQueryFilter::new()
        .with_name("Spam")
        .with_is_active(true);

    let val = serde_json::to_value(&filter).expect("serialize SieveScriptQueryFilter");
    assert_eq!(val["name"], "Spam");
    assert_eq!(val["isActive"], true);

    let back: SieveScriptQueryFilter =
        serde_json::from_value(val).expect("deserialize SieveScriptQueryFilter");
    assert_eq!(back, filter);
}

#[test]
fn sieve_validate_request_and_response_roundtrip() {
    let req = SieveScriptValidateRequest::new("acc1")
        .with_blob_id("blob_123")
        .with_content("require [\"fileinto\"]; fileinto \"Spam\";");

    let val_req = serde_json::to_value(&req).expect("serialize SieveScriptValidateRequest");
    assert_eq!(val_req["accountId"], "acc1");
    assert_eq!(val_req["blobId"], "blob_123");
    assert_eq!(
        val_req["content"],
        "require [\"fileinto\"]; fileinto \"Spam\";"
    );

    let back_req: SieveScriptValidateRequest =
        serde_json::from_value(val_req).expect("deserialize SieveScriptValidateRequest");
    assert_eq!(back_req, req);

    let resp_valid = SieveScriptValidateResponse::valid("acc1");
    let val_valid =
        serde_json::to_value(&resp_valid).expect("serialize SieveScriptValidateResponse");
    assert_eq!(val_valid["accountId"], "acc1");
    assert_eq!(val_valid["isValid"], true);
    assert!(val_valid.get("error").is_none());

    let err = SieveScriptValidateError::new()
        .with_description("Unknown action 'discard-all'")
        .with_line_number(4)
        .with_column_number(12)
        .with_action("discard-all");

    let resp_invalid = SieveScriptValidateResponse::invalid("acc1", err);
    let val_invalid =
        serde_json::to_value(&resp_invalid).expect("serialize invalid SieveScriptValidateResponse");
    assert_eq!(val_invalid["accountId"], "acc1");
    assert_eq!(val_invalid["isValid"], false);
    assert_eq!(
        val_invalid["error"]["description"],
        "Unknown action 'discard-all'"
    );
    assert_eq!(val_invalid["error"]["lineNumber"], 4);
    assert_eq!(val_invalid["error"]["columnNumber"], 12);
    assert_eq!(val_invalid["error"]["action"], "discard-all");

    let back_invalid: SieveScriptValidateResponse =
        serde_json::from_value(val_invalid).expect("deserialize invalid response");
    assert_eq!(back_invalid, resp_invalid);
}

#[test]
fn sieve_constants_and_set_error_coverage() {
    assert_eq!(CAPABILITY_SIEVE, "urn:ietf:params:jmap:sieve");
    assert_eq!(
        sieve_set_error::CANNOT_DELETE_ACTIVE_SCRIPT,
        "cannotDeleteActiveScript"
    );
    assert_eq!(
        sieve_set_error::DUPLICATE_SCRIPT_NAME,
        "duplicateScriptName"
    );
    assert_eq!(sieve_set_error::INVALID_SIEVE, "invalidSieve");
    assert_eq!(
        sieve_set_error::MAX_NUMBER_SCRIPTS_EXCEEDED,
        "maxNumberScriptsExceeded"
    );
    assert_eq!(
        sieve_set_error::MAX_SIZE_SCRIPT_EXCEEDED,
        "maxSizeScriptExceeded"
    );
    assert_eq!(
        sieve_set_error::MULTIPLE_ACTIVE_SCRIPTS,
        "multipleActiveScripts"
    );
}

#[test]
fn session_sieve_capability_accessor() {
    let session = Session::new(
        "user@example.com",
        "https://api.example.com/",
        "https://download.example.com/{blobId}",
        "https://upload.example.com/",
        "s_state_1",
    )
    .with_capability(
        CAPABILITY_SIEVE,
        json!({
            "maxSizeScript": 32768,
            "maxNumberScripts": 5,
            "implementation": "ManageSieve 1.0",
            "sieveExtensions": ["fileinto", "reject"]
        }),
    );

    let sieve_cap = session.sieve_capability().expect("typed sieve capability");
    assert_eq!(sieve_cap.max_size_script, 32768);
    assert_eq!(sieve_cap.max_number_scripts, Some(5));
    assert_eq!(sieve_cap.implementation.as_deref(), Some("ManageSieve 1.0"));
    assert_eq!(
        sieve_cap.sieve_extensions,
        vec!["fileinto".to_string(), "reject".to_string()]
    );
}
