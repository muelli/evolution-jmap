// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::methods::{Comparator, QueryRequest, SetRequest};
use jmap_proto::sieve::{
    SieveScript, SieveScriptQueryFilter, SieveScriptSetRequest, sieve_set_error,
};
use serde_json::json;

#[test]
fn sieve_create_with_omitted_name_assigns_server_chosen_name() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let uploaded = client
        .upload_blob(&account_id, "application/sieve", b"keep;\n".to_vec())
        .unwrap();

    let script1 = SieveScript::new("", uploaded.blob_id.clone());
    let req1 =
        SieveScriptSetRequest::new(SetRequest::new(account_id.clone()).create("s1", script1));
    let res1 = client.sieve_script_set(&req1).unwrap();
    let created1 = res1.created.unwrap();
    let assigned_name1 = created1.get("s1").unwrap().name.clone();
    assert!(
        !assigned_name1.is_empty(),
        "Server must assign a name when omitted per RFC 9661 Section 2.1"
    );

    let script2 = SieveScript::new("", uploaded.blob_id);
    let req2 =
        SieveScriptSetRequest::new(SetRequest::new(account_id.clone()).create("s2", script2));
    let res2 = client.sieve_script_set(&req2).unwrap();
    let created2 = res2.created.unwrap();
    let assigned_name2 = created2.get("s2").unwrap().name.clone();
    assert!(
        !assigned_name2.is_empty(),
        "Server must assign a name when omitted on second script"
    );
    assert_ne!(
        assigned_name1, assigned_name2,
        "Server-assigned names must be unique"
    );
}

#[test]
fn sieve_set_rejects_both_activate_and_deactivate_arguments() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let uploaded = client
        .upload_blob(&account_id, "application/sieve", b"keep;\n".to_vec())
        .unwrap();

    let script = SieveScript::new("my-script", uploaded.blob_id);
    let created = client.sieve_script_create(&account_id, &script).unwrap();
    let script_id = created.id.unwrap();

    let set_args = json!({
        "accountId": account_id,
        "onSuccessActivateScript": script_id,
        "onSuccessDeactivateScript": true
    });

    let res = client.single_call(
        &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:sieve"],
        "SieveScript/set",
        &set_args,
    );
    let err = res.expect_err("server must reject both activate and deactivate arguments");
    match err {
        jmap_client::Error::Method(method_err) => {
            assert_eq!(method_err.error_type, "invalidArguments");
            assert!(
                method_err
                    .description
                    .as_deref()
                    .is_some_and(|d| d.contains("mutually exclusive")),
                "Expected mutually exclusive description, got {:?}",
                method_err.description
            );
        }
        other => panic!("expected Error::Method(invalidArguments), got {other:?}"),
    }
}

#[test]
fn sieve_destroy_active_script_fails_with_sieve_is_active() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let uploaded = client
        .upload_blob(&account_id, "application/sieve", b"keep;\n".to_vec())
        .unwrap();

    let script = SieveScript::new("test", uploaded.blob_id);
    let created = client.sieve_script_create(&account_id, &script).unwrap();
    let script_id = created.id.unwrap();

    client
        .sieve_script_activate(&account_id, &script_id)
        .unwrap();

    // Destroying without deactivating must fail with sieveIsActive
    let fail_req = SieveScriptSetRequest::new(
        SetRequest::<SieveScript>::new(account_id.clone()).destroy(script_id.clone()),
    );
    let fail_res = client.sieve_script_set(&fail_req).unwrap();
    assert!(
        fail_res.not_destroyed.is_some_and(|nd| {
            nd.get(&script_id)
                .is_some_and(|err| err.error_type == sieve_set_error::SIEVE_IS_ACTIVE)
        }),
        "Destroying active script without deactivation must fail with sieveIsActive"
    );

    // Destroying with onSuccessDeactivateScript: true in same set also fails with sieveIsActive
    // because RFC 9661 Section 2.4 runs onSuccessDeactivateScript after destroy.
    let fail_req2 = SieveScriptSetRequest::new(
        SetRequest::<SieveScript>::new(account_id.clone()).destroy(script_id.clone()),
    )
    .deactivating();

    let fail_res2 = client.sieve_script_set(&fail_req2).unwrap();
    assert!(
        fail_res2.not_destroyed.is_some_and(|nd| {
            nd.get(&script_id)
                .is_some_and(|err| err.error_type == sieve_set_error::SIEVE_IS_ACTIVE)
        }),
        "Destroying active script with onSuccessDeactivateScript in same set must fail with sieveIsActive"
    );

    // Separate deactivation first succeeds, followed by destroy
    let deact_req = SieveScriptSetRequest::new(SetRequest::<SieveScript>::new(account_id.clone()))
        .deactivating();
    client.sieve_script_set(&deact_req).unwrap();

    let destroy_req = SieveScriptSetRequest::new(
        SetRequest::<SieveScript>::new(account_id).destroy(script_id.clone()),
    );
    let destroy_res = client.sieve_script_set(&destroy_req).unwrap();
    assert!(
        destroy_res
            .destroyed
            .is_some_and(|d| d.contains(&script_id)),
        "Destroying deactivated script must succeed"
    );
}

#[test]
fn sieve_query_filter_name_is_case_insensitive() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let uploaded = client
        .upload_blob(&account_id, "application/sieve", b"keep;\n".to_vec())
        .unwrap();

    let script = SieveScript::new("MyFilter", uploaded.blob_id);
    let created = client.sieve_script_create(&account_id, &script).unwrap();
    let script_id = created.id.unwrap();

    let ids = client
        .sieve_script_query(
            &account_id,
            SieveScriptQueryFilter::new().with_name("myfilter"),
        )
        .unwrap();
    assert!(
        ids.contains(&script_id),
        "Filter name matching must be case-insensitive per RFC 9661 Section 2.5"
    );
}

#[test]
fn sieve_query_sorts_by_name_and_is_active() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let account_id = server.account_id();

    let blob = client
        .upload_blob(&account_id, "application/sieve", b"keep;\n".to_vec())
        .unwrap();

    let s1 = client
        .sieve_script_create(&account_id, &SieveScript::new("Beta", blob.blob_id.clone()))
        .unwrap();
    let s2 = client
        .sieve_script_create(&account_id, &SieveScript::new("Alpha", blob.blob_id))
        .unwrap();

    let id1 = s1.id.unwrap();
    let id2 = s2.id.unwrap();

    // Sort by name ascending
    let query_req = QueryRequest::<SieveScriptQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::ascending("name")]);
    let query_res = client.sieve_script_query_request(&query_req).unwrap();
    assert_eq!(query_res.ids, vec![id2.clone(), id1.clone()]);

    // Sort by name descending
    let query_req_desc = QueryRequest::<SieveScriptQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::descending("name")]);
    let query_res_desc = client.sieve_script_query_request(&query_req_desc).unwrap();
    assert_eq!(query_res_desc.ids, vec![id1.clone(), id2.clone()]);

    // Activate id2
    client.sieve_script_activate(&account_id, &id2).unwrap();

    // Sort by isActive ascending (false first, then true)
    let query_active_asc = QueryRequest::<SieveScriptQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::ascending("isActive")]);
    let query_res_active_asc = client
        .sieve_script_query_request(&query_active_asc)
        .unwrap();
    assert_eq!(query_res_active_asc.ids, vec![id1.clone(), id2.clone()]);

    // Sort by isActive descending (true first, then false)
    let query_active_desc = QueryRequest::<SieveScriptQueryFilter>::new(account_id.clone())
        .sort(vec![Comparator::descending("isActive")]);
    let query_res_active_desc = client
        .sieve_script_query_request(&query_active_desc)
        .unwrap();
    assert_eq!(query_res_active_desc.ids, vec![id2, id1]);
}
