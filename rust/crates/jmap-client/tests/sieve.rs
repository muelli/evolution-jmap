// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Sieve (RFC 9661): capability detection, `SieveScript/get`, `/set`,
//! `/query` and `/validate`. No Evolution filters UI consumes this yet; the
//! wiring is a separate increment.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::blob::{BlobUploadRequest, UploadBlob};
use jmap_proto::error::{method, set};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_SIEVE};
use jmap_proto::sieve::{
    SieveScript, SieveScriptQueryFilter, SieveScriptValidateRequest, sieve_set_error,
};
use serde_json::json;

/// A server advertises `urn:ietf:params:jmap:sieve` both at session level and
/// on the account, the same way every other capability does, and the typed
/// capability parses (its `maxSizeScript` property is mandatory per RFC 9661
/// §1.1, unlike the empty placeholder object most other capabilities here
/// advertise).
#[test]
fn sieve_capability_is_advertised_and_resolves_to_the_account() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let capability = client.session().sieve_capability();
    assert!(capability.is_some());
    assert!(capability.unwrap().max_size_script > 0);
    assert_eq!(
        client.session().resolve_primary_account(CAPABILITY_SIEVE),
        Some(&account_id)
    );
}

/// A freshly started account has no Sieve scripts (unlike `Quota`, RFC 9661
/// scripts are client-created), so `ids: null` returns an empty list rather
/// than a seeded fixture.
#[test]
fn sieve_script_get_returns_no_scripts_on_a_fresh_account() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let scripts = client.sieve_scripts(&account_id).unwrap();
    assert!(scripts.is_empty());
}

/// `SieveScript/get` with an id that names nothing answers `notFound`, the
/// same as every other `/get` method (RFC 8620 §5.1).
#[test]
fn sieve_script_get_reports_unknown_ids_as_not_found() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let arguments = client
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_SIEVE],
            "SieveScript/get",
            &jmap_proto::methods::GetRequest::ids(account_id.clone(), ["nonexistent"]),
        )
        .unwrap();
    let response: jmap_proto::methods::GetResponse<jmap_proto::sieve::SieveScript> =
        serde_json::from_value(arguments).unwrap();
    assert!(response.list.is_empty());
    assert_eq!(
        response.not_found,
        vec![jmap_proto::Id::from("nonexistent")]
    );
}

/// `SieveScript/set` create stores the script, stamps a server-set id, and
/// leaves it inactive (RFC 9661 §2.4).
#[test]
fn sieve_script_create_stores_an_inactive_script() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let script = SieveScript::new("vacation", "B1");
    let created = client.sieve_script_create(&account_id, &script).unwrap();

    assert!(created.id.is_some());
    assert_eq!(created.name, "vacation");
    assert!(!created.is_active);

    let scripts = client.sieve_scripts(&account_id).unwrap();
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0].id, created.id);
}

/// A script name is unique per account (RFC 9661 §2.4): a second create
/// naming one already in use is `alreadyExists`, not silently accepted.
#[test]
fn sieve_script_create_rejects_a_duplicate_name() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();

    match client.sieve_script_create(&account_id, &SieveScript::new("vacation", "B2")) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, set::ALREADY_EXISTS),
        other => panic!("expected Set error, got {other:?}"),
    }
}

/// A plain JSON Patch update renames a script.
#[test]
fn sieve_script_update_renames() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let id = created.id.unwrap();

    client
        .sieve_script_update(&account_id, &id, json!({"name": "out-of-office"}))
        .unwrap();

    let scripts = client.sieve_scripts(&account_id).unwrap();
    assert_eq!(scripts[0].name, "out-of-office");
}

/// `isActive` is server-set (RFC 9661 §2.1): a direct update patch touching
/// it is `invalidProperties`, not a silent activation.
#[test]
fn sieve_script_update_rejects_a_direct_is_active_patch() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let id = created.id.unwrap();

    match client.sieve_script_update(&account_id, &id, json!({"isActive": true})) {
        Err(Error::Set(set_error)) => assert_eq!(set_error.error_type, set::INVALID_PROPERTIES),
        other => panic!("expected Set error, got {other:?}"),
    }
}

/// Destroying the active script is refused (`sieveIsActive`, RFC 9661
/// §2.4) until it is deactivated; destroying an inactive one works.
#[test]
fn sieve_script_destroy_refuses_the_active_script_until_deactivated() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let created = client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let id = created.id.unwrap();
    client.sieve_script_activate(&account_id, &id).unwrap();

    match client.sieve_script_destroy(&account_id, &id) {
        Err(Error::Set(set_error)) => {
            assert_eq!(set_error.error_type, sieve_set_error::SIEVE_IS_ACTIVE)
        }
        other => panic!("expected Set error, got {other:?}"),
    }

    client.sieve_script_deactivate(&account_id).unwrap();
    client.sieve_script_destroy(&account_id, &id).unwrap();
    assert!(client.sieve_scripts(&account_id).unwrap().is_empty());
}

/// Activating a second script deactivates whatever was active before,
/// since RFC 9661 never allows two scripts active at once.
#[test]
fn sieve_script_activate_switches_the_previous_one_off() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let a = client
        .sieve_script_create(&account_id, &SieveScript::new("a", "B1"))
        .unwrap()
        .id
        .unwrap();
    let b = client
        .sieve_script_create(&account_id, &SieveScript::new("b", "B2"))
        .unwrap()
        .id
        .unwrap();

    client.sieve_script_activate(&account_id, &a).unwrap();
    client.sieve_script_activate(&account_id, &b).unwrap();

    let scripts = client.sieve_scripts(&account_id).unwrap();
    let active: Vec<_> = scripts
        .iter()
        .filter(|script| script.is_active)
        .map(|script| script.id.clone().unwrap())
        .collect();
    assert_eq!(active, vec![b]);
}

/// `SieveScript/query` with no filter (RFC 9661 §2.5) returns every script,
/// the same as `sieve_scripts` but as bare ids.
#[test]
fn sieve_script_query_with_no_filter_returns_every_script() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let a = client
        .sieve_script_create(&account_id, &SieveScript::new("a", "B1"))
        .unwrap()
        .id
        .unwrap();
    let b = client
        .sieve_script_create(&account_id, &SieveScript::new("b", "B2"))
        .unwrap()
        .id
        .unwrap();

    let mut ids = client
        .sieve_script_query(&account_id, SieveScriptQueryFilter::new())
        .unwrap();
    ids.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(ids, expected);
}

/// Filtering by `name` (RFC 9661 §2.5) narrows to scripts whose name
/// contains the given substring.
#[test]
fn sieve_script_query_filters_by_name() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", "B1"))
        .unwrap();
    let out_of_office = client
        .sieve_script_create(&account_id, &SieveScript::new("out-of-office", "B2"))
        .unwrap()
        .id
        .unwrap();

    let ids = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_name("out-of"),
        )
        .unwrap();
    assert_eq!(ids, vec![out_of_office]);
}

/// Filtering by `isActive` (RFC 9661 §2.5) narrows to just the one active
/// script, the same the account's own `SieveScript/set` invariant only ever
/// allows one of.
#[test]
fn sieve_script_query_filters_by_is_active() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let a = client
        .sieve_script_create(&account_id, &SieveScript::new("a", "B1"))
        .unwrap()
        .id
        .unwrap();
    client
        .sieve_script_create(&account_id, &SieveScript::new("b", "B2"))
        .unwrap();
    client.sieve_script_activate(&account_id, &a).unwrap();

    let ids = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_is_active(true),
        )
        .unwrap();
    assert_eq!(ids, vec![a]);
}

/// `SieveScript/validate` (RFC 9661 section 2.6) validating raw `content` is
/// always `isValid: true`: this mock has no real Sieve parser, the same
/// deliberate limitation `SieveScript/set` already has (it never produces
/// `invalidSieve`).
#[test]
fn sieve_script_validate_by_content_is_always_valid() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let response = client
        .sieve_script_validate(
            &SieveScriptValidateRequest::new(account_id).with_content("not actually sieve at all"),
        )
        .unwrap();
    assert!(response.is_valid);
    assert!(response.error.is_none());
}

/// Validating by `blobId` resolves the blob against the account's real
/// store, the one mechanical check this mock can do without a parser.
#[test]
fn sieve_script_validate_by_blob_id_resolves_a_real_blob() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let uploaded = client
        .blob_upload(&BlobUploadRequest::new(account_id.clone()).create_blob(
            "b0",
            UploadBlob::from_text("require [\"fileinto\"];", "text/plain"),
        ))
        .unwrap();
    let blob_id = uploaded.created.unwrap().get("b0").unwrap().id.clone();

    let response = client
        .sieve_script_validate(&SieveScriptValidateRequest::new(account_id).with_blob_id(blob_id))
        .unwrap();
    assert!(response.is_valid);
}

/// An unknown `blobId` is `invalidArguments`: resolving the reference is
/// mechanical and this mock does check it, unlike script content.
#[test]
fn sieve_script_validate_rejects_an_unknown_blob_id() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    match client.sieve_script_validate(
        &SieveScriptValidateRequest::new(account_id).with_blob_id("nonexistent"),
    ) {
        Err(Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, method::INVALID_ARGUMENTS)
        }
        other => panic!("expected Method error, got {other:?}"),
    }
}

/// Validating by `id` resolves an existing script's own `blobId`.
#[test]
fn sieve_script_validate_by_id_resolves_the_scripts_own_blob() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let uploaded = client
        .blob_upload(&BlobUploadRequest::new(account_id.clone()).create_blob(
            "b0",
            UploadBlob::from_text("require [\"fileinto\"];", "text/plain"),
        ))
        .unwrap();
    let blob_id = uploaded.created.unwrap().get("b0").unwrap().id.clone();
    let script = client
        .sieve_script_create(&account_id, &SieveScript::new("vacation", blob_id))
        .unwrap();

    let response = client
        .sieve_script_validate(
            &SieveScriptValidateRequest::new(account_id).with_id(script.id.unwrap()),
        )
        .unwrap();
    assert!(response.is_valid);
}

/// RFC 9661 section 2.6 takes exactly one of `id`, `blobId` or `content`;
/// giving none is `invalidArguments`, the same as giving more than one.
#[test]
fn sieve_script_validate_requires_exactly_one_source() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    match client.sieve_script_validate(&SieveScriptValidateRequest::new(account_id.clone())) {
        Err(Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, method::INVALID_ARGUMENTS)
        }
        other => panic!("expected Method error, got {other:?}"),
    }

    match client.sieve_script_validate(
        &SieveScriptValidateRequest::new(account_id)
            .with_blob_id("b1")
            .with_content("require [\"fileinto\"];"),
    ) {
        Err(Error::Method(method_error)) => {
            assert_eq!(method_error.error_type, method::INVALID_ARGUMENTS)
        }
        other => panic!("expected Method error, got {other:?}"),
    }
}
