// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP Sieve (RFC 9661): capability detection and `SieveScript/get`. No
//! Evolution filters UI consumes this yet; the wiring is a separate
//! increment.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_SIEVE};

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
